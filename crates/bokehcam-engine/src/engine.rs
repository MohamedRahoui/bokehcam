//! The worker: owns the devices and moves frames.
//!
//! Threads:
//!   * worker    uploads, runs the model, renders and writes frames; owns all state,
//!   * capture   opens the camera, waits for frames and decodes them,
//!   * watcher   learns from the virtual camera whether any app is reading it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use bokehcam_core::Config;
use bokehcam_core::schema::Apply;
use bokehcam_gpu::{FrameParams, Gpu, Renderer, Sizes};
use bokehcam_infer::{Choice, Segmenter};
use bokehcam_media::{CaptureFormat, Decoder, Frame, MediaError};

use crate::EngineError;
use crate::devices::{Devices, FrameSink, SystemDevices};
use crate::status::{Preview, State, Status, Timer};

/// How long to wait before retrying after a failure (camera busy, device missing ...).
const RETRY: Duration = Duration::from_secs(2);
/// No frame for this long means the camera has stalled.
const FRAME_TIMEOUT: Duration = Duration::from_secs(4);
const CONFIG_POLL: Duration = Duration::from_millis(250);
/// The model may use this share of the time between two frames.
const MODEL_BUDGET: f32 = 0.35;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the whole engine down with poisoned locks.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Shared {
    stop: AtomicBool,
    config: Mutex<Config>,
    /// A config a front end asked for, not yet applied.
    pending: Mutex<Option<Config>>,
    status: Mutex<Status>,
    in_use: AtomicBool,
    last_use: Mutex<Option<Instant>>,
    preview_size: Mutex<Option<(u32, u32)>>,
    preview: Mutex<Preview>,
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    fn set_state(&self, state: State) {
        let mut status = lock(&self.status);
        if status.state != state {
            match &state {
                State::Problem(problem) => tracing::error!("{problem}"),
                other => tracing::info!("{other:?}"),
            }
            status.state = state;
        }
    }

    /// Sleep, waking early when asked to stop. Returns false when stopping.
    fn pause(&self, duration: Duration) -> bool {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            if self.stopping() {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
        !self.stopping()
    }

    /// Should the physical camera be on?
    fn wanted(&self, config: &Config) -> bool {
        if !config.output.on_demand || self.in_use.load(Ordering::Relaxed) || lock(&self.preview_size).is_some() {
            return true;
        }
        lock(&self.last_use).is_some_and(|t| t.elapsed().as_secs_f64() < config.output.idle_timeout)
    }
}

/// A handle to the running pipeline.
pub struct Engine {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Engine {
    /// Start the pipeline. When `config_path` is given, that file is watched
    /// for changes and receives configs set through [`Self::set_config`].
    pub fn start(config: Config, config_path: Option<PathBuf>) -> Self {
        Self::start_with(config, config_path, Arc::new(SystemDevices))
    }

    /// Start the pipeline on the given devices.
    pub(crate) fn start_with(config: Config, config_path: Option<PathBuf>, devices: Arc<dyn Devices>) -> Self {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            config: Mutex::new(config),
            pending: Mutex::new(None),
            status: Mutex::new(Status::default()),
            in_use: AtomicBool::new(false),
            last_use: Mutex::new(None),
            preview_size: Mutex::new(None),
            preview: Mutex::new(Preview::default()),
        });
        let worker = thread::Builder::new()
            .name("bokehcam-worker".into())
            .spawn({
                let shared = shared.clone();
                move || worker(&shared, config_path.as_deref(), &*devices)
            })
            .expect("cannot start a thread");
        Self { shared, worker: Mutex::new(Some(worker)) }
    }

    /// The configuration currently in effect.
    pub fn config(&self) -> Config {
        self.shared.config()
    }

    /// Apply a new (already validated) configuration. It takes effect within
    /// a fraction of a second and is written to the config file.
    pub fn set_config(&self, config: Config) {
        *lock(&self.shared.pending) = Some(config);
    }

    pub fn status(&self) -> Status {
        let mut status = lock(&self.shared.status).clone();
        status.in_use = self.shared.in_use.load(Ordering::Relaxed);
        status
    }

    /// Ask for preview pictures of this size (`None` to stop). While a
    /// preview is requested the camera stays on.
    pub fn set_preview(&self, size: Option<(u32, u32)>) {
        *lock(&self.shared.preview_size) = size;
    }

    /// The latest preview picture, if it is newer than `after`.
    pub fn preview(&self, after: u64) -> Option<Preview> {
        let preview = lock(&self.shared.preview);
        (preview.sequence > after).then(|| preview.clone())
    }

    /// Ask the pipeline to stop (returns immediately).
    pub fn request_stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }

    /// Stop the pipeline and release the devices.
    pub fn stop(&self) {
        self.request_stop();
        if let Some(worker) = lock(&self.worker).take() {
            let _ = worker.join();
        }
    }

    pub fn is_running(&self) -> bool {
        lock(&self.worker).as_ref().is_some_and(|w| !w.is_finished())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

// --- worker -------------------------------------------------------------------------

enum Next {
    /// The output configuration changed: reopen the virtual camera.
    Restart,
    Exit,
}

struct Context<'a> {
    shared: &'a Shared,
    gpu: &'a Gpu,
    devices: &'a dyn Devices,
    config_path: Option<&'a Path>,
    config_seen: Option<SystemTime>,
    /// The loaded model, kept across camera restarts.
    segmenter: Option<Segmenter>,
    /// The (model, backend, threads, fps) the segmenter was loaded for.
    segmenter_key: Option<(String, String, u32, u32)>,
}

fn worker(shared: &Shared, config_path: Option<&Path>, devices: &dyn Devices) {
    let gpu = loop {
        match Gpu::new() {
            Ok(gpu) => break gpu,
            Err(e) => {
                shared.set_state(State::Problem(e.to_string()));
                if !shared.pause(RETRY * 2) {
                    return;
                }
            }
        }
    };
    lock(&shared.status).graphics = format!("{} ({})", gpu.info().name, gpu.info().backend);
    let mut context = Context {
        shared,
        gpu: &gpu,
        devices,
        config_path,
        config_seen: config_path.and_then(modified),
        segmenter: None,
        segmenter_key: None,
    };
    while !shared.stopping() {
        match context.serve() {
            Ok(Next::Restart) => continue,
            Ok(Next::Exit) => break,
            Err(e) => {
                shared.set_state(State::Problem(e.to_string()));
                // A changed config may be the fix; pick it up while waiting.
                context.poll_config();
                if !shared.pause(RETRY) {
                    break;
                }
            }
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Context<'_> {
    /// Pick up a new configuration from a front end or from the config file.
    /// Returns what has to restart, if anything changed.
    fn poll_config(&mut self) -> Option<Apply> {
        let pending = lock(&self.shared.pending).take();
        let new = match pending {
            Some(config) => {
                if let Some(path) = self.config_path {
                    match config.save(path) {
                        Ok(()) => self.config_seen = modified(path),
                        Err(e) => tracing::warn!("settings applied but not saved: {e}"),
                    }
                }
                config
            }
            None => {
                let path = self.config_path?;
                let seen = modified(path);
                if seen == self.config_seen {
                    return None;
                }
                self.config_seen = seen;
                match Config::load_or_default(path) {
                    Ok(config) => config,
                    Err(e) => {
                        tracing::error!("config file not applied, keeping the previous settings: {e}");
                        return None;
                    }
                }
            }
        };
        let level = lock(&self.shared.config).apply_level(&new)?;
        *lock(&self.shared.config) = new;
        tracing::info!("settings updated");
        Some(level)
    }

    /// Serve one virtual-camera configuration until it changes or we stop.
    fn serve(&mut self) -> Result<Next, EngineError> {
        let shared = self.shared;
        let config = shared.config();
        let output = &config.output;
        let vcam = self.devices.open_virtual_camera(&output.device, output.width, output.height, config.camera.fps)?;
        let vcam = &*vcam;
        lock(&shared.status).output = format!("{}, {}x{}", vcam.device(), output.width, output.height);
        tracing::info!(
            "virtual camera {}: {}x{}, {}",
            vcam.device(),
            output.width,
            output.height,
            if output.on_demand { "camera on demand" } else { "camera always on" }
        );
        shared.in_use.store(false, Ordering::Relaxed);
        if output.on_demand {
            vcam.watch_consumers()?;
        }

        let watching = AtomicBool::new(output.on_demand);
        thread::scope(|scope| {
            scope.spawn(|| {
                while watching.load(Ordering::Relaxed) {
                    match vcam.wait_consumer_change(Duration::from_millis(300)) {
                        Some(true) => shared.in_use.store(true, Ordering::Relaxed),
                        // The first report is "nobody"; only a real stop starts the idle timer.
                        Some(false) if shared.in_use.swap(false, Ordering::Relaxed) => {
                            *lock(&shared.last_use) = Some(Instant::now());
                        }
                        _ => {}
                    }
                }
            });
            let result = self.serve_loop(vcam);
            watching.store(false, Ordering::Relaxed);
            result
        })
    }

    fn serve_loop(&mut self, vcam: &dyn FrameSink) -> Result<Next, EngineError> {
        let shared = self.shared;
        loop {
            if shared.stopping() {
                return Ok(Next::Exit);
            }
            if self.poll_config() == Some(Apply::ReopenOutput) {
                return Ok(Next::Restart);
            }
            if !shared.wanted(&shared.config()) {
                shared.set_state(State::Idle);
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            match self.stream(vcam) {
                Ok(Some(next)) => return Ok(next),
                Ok(None) => {}
                // Camera trouble is retried here, so apps keep the virtual camera open.
                Err(e) => {
                    shared.set_state(State::Problem(e.to_string()));
                    if !shared.pause(RETRY) {
                        return Ok(Next::Exit);
                    }
                }
            }
        }
    }

    /// Make sure the configured model is loaded. A missing or failing model
    /// is reported but not fatal: the picture is delivered without blur.
    fn ensure_segmenter(&mut self, config: &Config) -> Option<String> {
        if config.background.mode != "blur" {
            self.segmenter = None;
            self.segmenter_key = None;
            lock(&self.shared.status).model = "off".into();
            return None;
        }
        let seg = &config.segmentation;
        let key = (seg.model.clone(), seg.backend.clone(), seg.threads, config.camera.fps);
        if self.segmenter.is_some() && self.segmenter_key.as_ref() == Some(&key) {
            return None;
        }
        self.segmenter = None;
        let budget = MODEL_BUDGET * 1000.0 / config.camera.fps as f32;
        match Segmenter::load(&seg.model, Choice::parse(&seg.backend), seg.threads as usize, budget) {
            Ok(segmenter) => {
                let description = format!(
                    "{} on {} ({:.1} ms)",
                    segmenter.manifest().name,
                    segmenter.backend().label(),
                    segmenter.typical_ms()
                );
                tracing::info!("model: {description}");
                lock(&self.shared.status).model = description;
                self.segmenter = Some(segmenter);
                self.segmenter_key = Some(key);
                None
            }
            Err(e) => {
                lock(&self.shared.status).model = format!("unavailable: {e}");
                Some(format!("background blur is off: {e}"))
            }
        }
    }

    /// Run the camera until nobody wants it (`Ok(None)`), something has to
    /// restart (`Ok(Some)`), or it fails.
    fn stream(&mut self, vcam: &dyn FrameSink) -> Result<Option<Next>, EngineError> {
        let shared = self.shared;
        let gpu = self.gpu;
        let devices = self.devices;
        let mut config = shared.config();
        let model_problem = self.ensure_segmenter(&config);

        let slot = FrameSlot::default();
        let capturing = AtomicBool::new(true);
        let (opened_tx, opened_rx) = mpsc::channel();
        let camera_config = config.camera.clone();

        thread::scope(|scope| {
            scope.spawn(|| capture(devices, &camera_config, &slot, &capturing, opened_tx));
            let result = (|| -> Result<Option<Next>, EngineError> {
                let format: CaptureFormat = opened_rx.recv().map_err(|_| MediaError::NoCamera)??;
                lock(&shared.status).camera =
                    format!("{}x{} {:?} at {:.0} fps", format.width, format.height, format.encoding, format.fps);
                shared.set_state(match &model_problem {
                    Some(problem) => State::Problem(problem.clone()),
                    None => State::Streaming,
                });

                let mut renderer: Option<Renderer> = None;
                let mut params = frame_params(&config, self.segmenter.is_some());
                let mut mask = Vec::new();
                let mut timer = Timer::default();
                let mut frames = 0u32;
                let mut since = Instant::now();
                let mut last_poll = Instant::now();
                let mut spare = None;

                loop {
                    let (frame, decode_ms) = slot.take(spare.take(), FRAME_TIMEOUT)?;
                    timer.record("decode", decode_ms);
                    timer.start();

                    let sizes = Sizes {
                        source: (frame.width, frame.height),
                        output: (config.output.width, config.output.height),
                        model: self.segmenter.as_ref().map(|s| s.input_size()),
                    };
                    if renderer.as_ref().map(Renderer::sizes) != Some(sizes) {
                        renderer = Some(Renderer::new(gpu, sizes));
                    }
                    let renderer = renderer.as_mut().expect("just created");

                    renderer.upload(gpu, &frame.rgba);
                    timer.lap("upload");
                    let model_input = renderer.prepare(gpu, &params)?;
                    timer.lap("frame");
                    if let (Some(image), Some(segmenter)) = (model_input, self.segmenter.as_mut()) {
                        segmenter.run(image, &mut mask)?;
                        renderer.set_mask(gpu, &mask);
                        timer.lap("model");
                    }
                    let yuyv = renderer.finish(gpu, &params)?;
                    timer.lap("render");
                    if let Err(e) = vcam.write(yuyv) {
                        tracing::warn!("{e}");
                    }
                    timer.lap("output");
                    spare = Some(frame);

                    let preview_size = *lock(&shared.preview_size);
                    if let Some((width, height)) = preview_size {
                        let rgba = renderer.preview(gpu, width, height)?;
                        let mut preview = lock(&shared.preview);
                        preview.width = width;
                        preview.height = height;
                        preview.rgba.clear();
                        preview.rgba.extend_from_slice(rgba);
                        preview.sequence += 1;
                        timer.lap("preview");
                    }

                    frames += 1;
                    let elapsed = since.elapsed();
                    if elapsed >= Duration::from_secs(1) {
                        let mut status = lock(&shared.status);
                        status.fps = frames as f32 / elapsed.as_secs_f32();
                        status.stages = timer.stages();
                        frames = 0;
                        since = Instant::now();
                    }

                    if shared.stopping() {
                        return Ok(Some(Next::Exit));
                    }
                    if last_poll.elapsed() >= CONFIG_POLL {
                        last_poll = Instant::now();
                        match self.poll_config() {
                            Some(Apply::ReopenOutput) => return Ok(Some(Next::Restart)),
                            Some(Apply::ReopenCamera) => return Ok(None),
                            Some(level) => {
                                config = shared.config();
                                if level == Apply::ReloadModel || config.background.mode == "blur" {
                                    if let Some(problem) = self.ensure_segmenter(&config) {
                                        shared.set_state(State::Problem(problem));
                                    } else {
                                        shared.set_state(State::Streaming);
                                    }
                                }
                                params = frame_params(&config, self.segmenter.is_some());
                                timer.clear();
                            }
                            None => {}
                        }
                        if !shared.wanted(&config) {
                            return Ok(None);
                        }
                    }
                }
            })();
            capturing.store(false, Ordering::Relaxed);
            let mut status = lock(&shared.status);
            status.fps = 0.0;
            status.stages.clear();
            result
        })
    }
}

fn frame_params(config: &Config, have_model: bool) -> FrameParams {
    FrameParams {
        zoom: config.framing.zoom as f32,
        center: (config.framing.center_x as f32, config.framing.center_y as f32),
        mirror: config.framing.mirror,
        blur: have_model && config.background.mode == "blur",
        blur_percent: config.background.blur as f32,
        temporal_smoothing: config.mask.temporal_smoothing as f32,
        edge_radius_percent: config.mask.edge_radius as f32,
        edge_eps: config.mask.edge_eps as f32,
        cutoff_low: config.mask.cutoff_low as f32,
        cutoff_high: config.mask.cutoff_high as f32,
        grow: config.mask.grow as f32,
        brightness: config.color.brightness as f32,
        contrast: config.color.contrast as f32,
        saturation: config.color.saturation as f32,
        gamma: config.color.gamma as f32,
    }
}

// --- capture thread -------------------------------------------------------------------

/// One-slot handoff from the capture thread: the worker always gets the
/// newest frame, and frame buffers are recycled instead of reallocated.
#[derive(Default)]
struct FrameSlot {
    state: Mutex<SlotState>,
    ready: Condvar,
}

#[derive(Default)]
struct SlotState {
    latest: Option<(Frame, f32)>,
    spare: Option<Frame>,
    failed: Option<MediaError>,
}

impl FrameSlot {
    /// Wait for the next frame, handing back the previous one for reuse.
    fn take(&self, recycle: Option<Frame>, timeout: Duration) -> Result<(Frame, f32), MediaError> {
        let mut state = lock(&self.state);
        if recycle.is_some() {
            state.spare = recycle;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = state.latest.take() {
                return Ok(frame);
            }
            if let Some(error) = state.failed.take() {
                return Err(error);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MediaError::Capture(std::io::Error::other("no frames arriving")));
            }
            state = self.ready.wait_timeout(state, remaining).unwrap_or_else(|p| p.into_inner()).0;
        }
    }
}

fn capture(
    devices: &dyn Devices,
    config: &bokehcam_core::config::Camera,
    slot: &FrameSlot,
    capturing: &AtomicBool,
    opened: mpsc::Sender<Result<CaptureFormat, MediaError>>,
) {
    // The camera is opened here because its handle must stay on one thread.
    let mut camera = match devices.open_camera(config) {
        Ok(camera) => {
            let _ = opened.send(Ok(camera.format()));
            camera
        }
        Err(e) => {
            let _ = opened.send(Err(e));
            return;
        }
    };
    let format = camera.format();
    let decoder = Decoder::new(format.encoding, format.width, format.height);
    let mut bad_frames = 0;
    while capturing.load(Ordering::Relaxed) {
        let packet = match camera.next_packet() {
            Ok(packet) => packet,
            Err(e) => {
                lock(&slot.state).failed = Some(e);
                slot.ready.notify_one();
                return;
            }
        };
        let mut frame = lock(&slot.state).spare.take().unwrap_or_default();
        let start = Instant::now();
        match decoder.decode(packet, &mut frame) {
            Ok(()) => bad_frames = 0,
            Err(e) => {
                // Webcams occasionally emit a corrupt JPEG; skip it.
                bad_frames += 1;
                lock(&slot.state).spare = Some(frame);
                if bad_frames > 30 {
                    lock(&slot.state).failed = Some(e);
                    slot.ready.notify_one();
                    return;
                }
                continue;
            }
        }
        let decode_ms = start.elapsed().as_secs_f32() * 1000.0;
        let mut state = lock(&slot.state);
        // An unconsumed frame is dropped (its buffer becomes the spare):
        // falling behind must never build up delay.
        if let Some((old, _)) = state.latest.replace((frame, decode_ms)) {
            state.spare = Some(old);
        }
        drop(state);
        slot.ready.notify_one();
    }
}
