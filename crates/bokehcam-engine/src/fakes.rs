//! Stand-ins for the camera, the virtual camera and the picture stages, so
//! tests can run an engine on a machine that has none of them.
//!
//! A test holds a [`Rig`]: through it the test plays the outside world (an
//! app starts reading, the camera breaks) and sees what the engine did to
//! its devices.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use bokehcam_core::config;
use bokehcam_gpu::{FrameParams, GpuError};
use bokehcam_infer::InferError;
use bokehcam_media::{CaptureFormat, Encoding, Frame, MediaError};

use crate::EngineError;
use crate::devices::{Devices, FrameSink, FrameSource};
use crate::picture::{ModelRequest, Picture, PictureStages};
use crate::status::Timer;

/// The size of the frames the fake camera delivers.
const CAMERA_SIZE: (u32, u32) = (64, 36);
/// The fake camera delivers a frame this often.
const FRAME_INTERVAL: Duration = Duration::from_millis(5);
/// A corrupt packet is cut down to this many bytes.
const CORRUPT_PACKET_BYTES: usize = 10;

/// One opening of the virtual camera, as the engine asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputOpening {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Default)]
struct RigState {
    camera_opens: u32,
    camera_is_open: bool,
    camera_open_problem: Option<String>,
    camera_fails_next_packet: bool,
    corrupt_packets_left: u32,
    output_openings: Vec<OutputOpening>,
    frames_written: u64,
    last_frame_bytes: usize,
    /// A change in whether an app is reading, not yet collected by the engine.
    consumer_report: Option<bool>,
    graphics_failures_left: u32,
    models_fail: bool,
    models_loaded: Vec<String>,
}

/// What a test controls and observes.
#[derive(Default)]
pub(crate) struct Rig {
    state: Mutex<RigState>,
    consumer_reported: Condvar,
}

impl Rig {
    /// A rig with the devices and picture stages that run on it.
    pub fn new() -> (Arc<Self>, Arc<FakeDevices>, Arc<FakeStages>) {
        let rig = Arc::new(Self::default());
        (rig.clone(), Arc::new(FakeDevices(rig.clone())), Arc::new(FakeStages(rig)))
    }

    fn state(&self) -> MutexGuard<'_, RigState> {
        self.state.lock().expect("the rig is not poisoned")
    }

    fn report_consumer(&self, reading: bool) {
        self.state().consumer_report = Some(reading);
        self.consumer_reported.notify_all();
    }

    // --- the outside world ---

    pub fn app_starts_reading(&self) {
        self.report_consumer(true);
    }

    pub fn app_stops_reading(&self) {
        self.report_consumer(false);
    }

    /// Opening the camera fails with this problem until [`Self::repair_camera`].
    pub fn break_camera(&self, problem: &str) {
        self.state().camera_open_problem = Some(problem.to_string());
    }

    pub fn repair_camera(&self) {
        self.state().camera_open_problem = None;
    }

    /// The open camera fails once, as when it is unplugged.
    pub fn fail_next_packet(&self) {
        self.state().camera_fails_next_packet = true;
    }

    /// The next `count` packets cannot be decoded.
    pub fn corrupt_next_packets(&self, count: u32) {
        self.state().corrupt_packets_left = count;
    }

    /// The next `count` attempts to open the picture stages fail.
    pub fn break_graphics(&self, count: u32) {
        self.state().graphics_failures_left = count;
    }

    /// Loading any model fails from now on.
    pub fn break_models(&self) {
        self.state().models_fail = true;
    }

    // --- what the engine did ---

    pub fn camera_opens(&self) -> u32 {
        self.state().camera_opens
    }

    pub fn camera_is_open(&self) -> bool {
        self.state().camera_is_open
    }

    pub fn corrupt_packets_left(&self) -> u32 {
        self.state().corrupt_packets_left
    }

    pub fn output_openings(&self) -> Vec<OutputOpening> {
        self.state().output_openings.clone()
    }

    pub fn frames_written(&self) -> u64 {
        self.state().frames_written
    }

    /// Size of the frame written last, in bytes.
    pub fn last_frame_bytes(&self) -> usize {
        self.state().last_frame_bytes
    }

    /// Every model loaded so far, by the name it was asked for.
    pub fn models_loaded(&self) -> Vec<String> {
        self.state().models_loaded.clone()
    }
}

pub(crate) struct FakeDevices(Arc<Rig>);

impl Devices for FakeDevices {
    fn open_camera(&self, _: &config::Camera) -> Result<Box<dyn FrameSource>, MediaError> {
        let mut state = self.0.state();
        if let Some(problem) = &state.camera_open_problem {
            return Err(MediaError::Device { device: "fake camera".into(), problem: problem.clone() });
        }
        state.camera_opens += 1;
        state.camera_is_open = true;
        let packet = vec![128; (CAMERA_SIZE.0 * CAMERA_SIZE.1 * 2) as usize];
        Ok(Box::new(FakeCamera { rig: self.0.clone(), packet }))
    }

    fn open_virtual_camera(
        &self,
        device: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        self.0.state().output_openings.push(OutputOpening { device: device.to_string(), width, height, fps });
        Ok(Box::new(FakeVirtualCamera(self.0.clone())))
    }
}

struct FakeCamera {
    rig: Arc<Rig>,
    /// One YUYV frame of mid grey.
    packet: Vec<u8>,
}

impl FrameSource for FakeCamera {
    fn format(&self) -> CaptureFormat {
        CaptureFormat { encoding: Encoding::Yuyv, width: CAMERA_SIZE.0, height: CAMERA_SIZE.1, fps: 30.0 }
    }

    fn next_packet(&mut self) -> Result<&[u8], MediaError> {
        thread::sleep(FRAME_INTERVAL);
        let mut state = self.rig.state();
        if std::mem::take(&mut state.camera_fails_next_packet) {
            return Err(MediaError::Capture(std::io::Error::other("the fake camera was unplugged")));
        }
        if state.corrupt_packets_left > 0 {
            state.corrupt_packets_left -= 1;
            return Ok(&self.packet[..CORRUPT_PACKET_BYTES]);
        }
        Ok(&self.packet)
    }
}

impl Drop for FakeCamera {
    fn drop(&mut self) {
        self.rig.state().camera_is_open = false;
    }
}

struct FakeVirtualCamera(Arc<Rig>);

impl FrameSink for FakeVirtualCamera {
    fn device(&self) -> &str {
        "fake virtual camera"
    }

    fn write(&self, yuyv: &[u8]) -> Result<(), MediaError> {
        let mut state = self.0.state();
        state.frames_written += 1;
        state.last_frame_bytes = yuyv.len();
        Ok(())
    }

    fn watch_consumers(&self) -> Result<(), MediaError> {
        // Like the real driver, the first report says that nobody is reading.
        self.0.report_consumer(false);
        Ok(())
    }

    fn wait_consumer_change(&self, timeout: Duration) -> Option<bool> {
        let deadline = Instant::now() + timeout;
        let mut state = self.0.state();
        loop {
            if let Some(reading) = state.consumer_report.take() {
                return Some(reading);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            state = self.0.consumer_reported.wait_timeout(state, remaining).expect("the rig is not poisoned").0;
        }
    }
}

pub(crate) struct FakeStages(Arc<Rig>);

impl PictureStages for FakeStages {
    fn open(&self) -> Result<Box<dyn Picture>, EngineError> {
        let mut state = self.0.state();
        if state.graphics_failures_left > 0 {
            state.graphics_failures_left -= 1;
            return Err(GpuError::NoAdapter.into());
        }
        Ok(Box::new(FakePicture { rig: self.0.clone(), frame: Vec::new(), preview: Vec::new() }))
    }
}

/// Produces frames of the right size and nothing more.
struct FakePicture {
    rig: Arc<Rig>,
    frame: Vec<u8>,
    preview: Vec<u8>,
}

impl Picture for FakePicture {
    fn graphics(&self) -> String {
        "fake graphics".into()
    }

    fn load_model(&mut self, request: &ModelRequest) -> Result<String, EngineError> {
        let mut state = self.rig.state();
        if state.models_fail {
            return Err(InferError::NotInstalled(request.model.clone()).into());
        }
        state.models_loaded.push(request.model.clone());
        Ok(format!("{} on fake graphics", request.model))
    }

    fn drop_model(&mut self) {}

    fn render(
        &mut self,
        _: &Frame,
        output: (u32, u32),
        _: &FrameParams,
        timer: &mut Timer,
    ) -> Result<&[u8], EngineError> {
        self.frame.resize((output.0 * output.1 * 2) as usize, 0);
        timer.lap("render");
        Ok(&self.frame)
    }

    fn preview(&mut self, width: u32, height: u32) -> Result<&[u8], EngineError> {
        self.preview.resize((width * height * 4) as usize, 0);
        Ok(&self.preview)
    }

    fn rest(&mut self) {}
}
