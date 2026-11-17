//! The `bokehcam` command: run the virtual camera and inspect the setup.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bokehcam_core::{Config, paths};
use bokehcam_engine::control::{Client, ControlError, ControlServer};
use bokehcam_engine::{Engine, Preview, State, Status};
use bokehcam_gpu::Gpu;
use bokehcam_infer::{Backend, Segmenter, manifest};
use bokehcam_media::{Camera, list_cameras, list_virtual_cameras};
use clap::{Parser, Subcommand};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(name = "bokehcam", version, about = "A virtual webcam with a sharp picture and background blur.")]
struct Cli {
    /// Config file (default: ~/.config/bokehcam/config.toml)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    /// Show debug messages
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the virtual camera (Ctrl+C to stop)
    Run {
        /// Print the frame rate and per-step timings every N seconds
        #[arg(long, value_name = "SECONDS")]
        stats: Option<f64>,
    },
    /// Show what the running bokehcam is doing
    Status,
    /// Print one setting, or all of them
    Get {
        /// e.g. background.blur
        key: Option<String>,
    },
    /// Change one setting (applies immediately if bokehcam is running)
    Set {
        /// e.g. background.blur
        key: String,
        value: String,
    },
    /// Check that everything bokehcam needs is present on this machine
    Doctor,
    /// List cameras and virtual cameras
    Devices {
        /// Also list the capture modes of the camera
        #[arg(long)]
        modes: bool,
        /// Also list the hardware controls of the camera
        #[arg(long)]
        controls: bool,
    },
    /// List, download or time the person-detection models
    Models {
        #[command(subcommand)]
        action: Option<ModelAction>,
    },
    /// Write the documented default config file
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Save one finished picture as a PNG (turns the camera on briefly)
    Snapshot {
        file: PathBuf,
        /// Frames to let pass first, so exposure and the outline settle
        #[arg(long, default_value_t = 30)]
        warmup: u64,
    },
}

#[derive(Subcommand)]
enum ModelAction {
    /// Show every known model and whether it is downloaded
    List,
    /// Download models (all of them when no name is given)
    Fetch { names: Vec<String> },
    /// Time every downloaded model on the processor and the graphics card
    Bench,
}

fn main() {
    let cli = Cli::parse();
    // The model runtime is chatty (every internal step at "info", harmless
    // model-format notes at "warn"); only pass on its errors.
    let level = if cli.verbose { tracing::Level::DEBUG } else { tracing::Level::INFO };
    let filter = Targets::new().with_default(level).with_target("ort", tracing::Level::ERROR);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false).compact())
        .with(filter)
        .init();
    if let Err(error) = run(cli) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let config_path = cli.config.clone().unwrap_or_else(paths::config_file);
    match cli.command {
        Command::Run { stats } => run_service(&config_path, stats),
        Command::Status => status(),
        Command::Get { key } => get(&config_path, key.as_deref()),
        Command::Set { key, value } => set(&config_path, &key, &value),
        Command::Doctor => doctor(&config_path),
        Command::Devices { modes, controls } => devices(&config_path, modes, controls),
        Command::Models { action } => match action.unwrap_or(ModelAction::List) {
            ModelAction::List => list_models(),
            ModelAction::Fetch { names } => fetch_models(&names),
            ModelAction::Bench => bench_models(),
        },
        Command::Init { force } => init(&config_path, force),
        Command::Snapshot { file, warmup } => snapshot(&config_path, &file, warmup),
    }
}

fn stop_flag() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)).context("cannot install the Ctrl+C handler")?;
    Ok(stop)
}

fn run_service(config_path: &Path, stats: Option<f64>) -> Result<()> {
    let config = Config::load_or_default(config_path)?;
    if !config_path.exists() {
        tracing::info!("no config file; using defaults (create one with `bokehcam init`)");
    }
    let stop = stop_flag()?;
    let engine = Arc::new(Engine::start(config, Some(config_path.to_path_buf())));
    let _control = ControlServer::start(engine.clone(), &paths::control_socket())?;
    let mut last_stats = Instant::now();
    while !stop.load(Ordering::Relaxed) && engine.is_running() {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(every) = stats
            && last_stats.elapsed().as_secs_f64() >= every
        {
            last_stats = Instant::now();
            let status = engine.status();
            if status.fps > 0.0 {
                tracing::info!("{}", timing_line(&status));
            }
        }
    }
    engine.stop();
    Ok(())
}

fn timing_line(status: &Status) -> String {
    let stages: Vec<String> = status.stages.iter().map(|s| format!("{} {:.1}", s.name, s.ms)).collect();
    format!("{:.1} fps, {:.1} ms per frame ({})", status.fps, status.frame_ms(), stages.join(", "))
}

/// A connection to the running bokehcam, or `None` when it is not running.
fn running() -> Result<Option<Client>> {
    match Client::connect(&paths::control_socket()) {
        Ok(client) => Ok(Some(client)),
        Err(ControlError::NotRunning) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn status() -> Result<()> {
    let Some(mut client) = running()? else {
        println!("bokehcam is not running (start it with `bokehcam run`)");
        return Ok(());
    };
    let status = client.status()?;
    let state = match &status.state {
        State::Starting => "starting".to_string(),
        State::Idle => "ready, camera off (no app is using the virtual camera)".to_string(),
        State::Streaming => "streaming".to_string(),
        State::Problem(problem) => format!("problem: {problem}"),
    };
    println!("state:           {state}");
    println!("virtual camera:  {}{}", status.output, if status.in_use { " (in use by an app)" } else { "" });
    println!("camera:          {}", if status.camera.is_empty() { "not opened yet" } else { &status.camera });
    println!("model:           {}", status.model);
    println!("graphics:        {}", status.graphics);
    if status.fps > 0.0 {
        println!("speed:           {}", timing_line(&status));
    }
    Ok(())
}

fn get(config_path: &Path, key: Option<&str>) -> Result<()> {
    let config = match running()? {
        Some(mut client) => client.config()?,
        None => Config::load_or_default(config_path)?,
    };
    match key {
        Some(key) => println!("{}", config.get(key)?),
        None => print!("{}", config.to_toml()),
    }
    Ok(())
}

fn set(config_path: &Path, key: &str, value: &str) -> Result<()> {
    // Accept numbers and true/false as they are; anything else is text.
    let parsed = value.parse::<toml::Value>().unwrap_or_else(|_| toml::Value::String(value.to_string()));
    match running()? {
        Some(mut client) => {
            let config = client.config()?.with(key, parsed)?;
            client.set_config(&config)?;
            println!("{key} = {} (applied)", config.get(key)?);
        }
        None => {
            let config = Config::load_or_default(config_path)?.with(key, parsed)?;
            config.save(config_path)?;
            println!("{key} = {} (saved; bokehcam is not running)", config.get(key)?);
        }
    }
    Ok(())
}

fn init(config_path: &Path, force: bool) -> Result<()> {
    if config_path.exists() && !force {
        bail!("{} already exists (use --force to overwrite)", config_path.display());
    }
    Config::default().save(config_path)?;
    println!("wrote {}", config_path.display());
    Ok(())
}

fn devices(config_path: &Path, modes: bool, controls: bool) -> Result<()> {
    let cameras = list_cameras();
    println!("Cameras:");
    for camera in &cameras {
        println!("  {}  {}", camera.path, camera.name);
    }
    if cameras.is_empty() {
        println!("  none found");
    }
    println!("Virtual cameras:");
    let virtual_cameras = list_virtual_cameras();
    for device in &virtual_cameras {
        println!("  {}  {}", device.path, device.name);
    }
    if virtual_cameras.is_empty() {
        println!("  none found (see `bokehcam doctor`)");
    }
    let device = Config::load_or_default(config_path)?.camera.device;
    if modes {
        println!("Capture modes of the camera in use:");
        for mode in Camera::modes(&device)? {
            let encoding = if mode.encoding_mjpeg { "Motion-JPEG" } else { "YUYV" };
            println!("  {:>4}x{:<4}  up to {:>3} fps  {encoding}", mode.width, mode.height, mode.max_fps);
        }
    }
    if controls {
        println!("Hardware controls of the camera in use (for [camera.controls]):");
        for control in Camera::controls(&device)? {
            let value = control.value.map_or("?".to_string(), |v| v.to_string());
            let note = if control.inactive { "  (locked by another control)" } else { "" };
            println!(
                "  {:<32} = {value:<6} range {}..{}, default {}{note}",
                control.name, control.min, control.max, control.default
            );
        }
    }
    Ok(())
}

fn list_models() -> Result<()> {
    for model in manifest::all() {
        let state = match model.installed_path() {
            Some(path) => format!("downloaded ({})", path.display()),
            None => "not downloaded".to_string(),
        };
        println!("{}  [{}]  {}", model.name, model.license, state);
        println!("    {}: {}", model.title, model.description);
    }
    println!("\nDownload with `bokehcam models fetch`. Set `segmentation.model` to a name, or \"auto\".");
    Ok(())
}

fn fetch_models(names: &[String]) -> Result<()> {
    let models = if names.is_empty() {
        manifest::all()
    } else {
        names.iter().map(|name| manifest::find(name)).collect::<Result<_, _>>()?
    };
    for model in models {
        let mut last = Instant::now();
        let path = model.fetch(|done, total| {
            if last.elapsed() > Duration::from_millis(500) {
                last = Instant::now();
                match total {
                    Some(total) => eprintln!("  {}: {} of {} MB", model.name, done >> 20, total >> 20),
                    None => eprintln!("  {}: {} MB", model.name, done >> 20),
                }
            }
        })?;
        println!("{}: {}", model.name, path.display());
    }
    Ok(())
}

fn bench_models() -> Result<()> {
    let gpu = bokehcam_infer::gpu_available();
    println!("Time for one run of each downloaded model (lower is better):");
    for model in manifest::all().into_iter().filter(|m| m.installed_path().is_some()) {
        let mut line = format!("  {:<20}", model.name);
        for (backend, enabled) in [(Backend::Cpu, true), (Backend::Gpu, gpu)] {
            if !enabled {
                continue;
            }
            match Segmenter::load_on(&model, backend, 2) {
                Ok(segmenter) => line += &format!("  {}: {:.1} ms", backend.label(), segmenter.typical_ms()),
                Err(e) => line += &format!("  {}: failed ({e})", backend.label()),
            }
        }
        println!("{line}");
    }
    if !gpu {
        println!("  (running models on the graphics card is not available on this machine)");
    }
    Ok(())
}

fn doctor(config_path: &Path) -> Result<()> {
    let mut problems = 0;
    let mut report = |ok: bool, text: String| {
        problems += !ok as u32;
        println!("{} {text}", if ok { "ok   " } else { "FAIL " });
    };

    match Config::load_or_default(config_path) {
        Ok(_) if config_path.exists() => report(true, format!("config: {}", config_path.display())),
        Ok(_) => report(true, "config: built-in defaults (no file yet; `bokehcam init` writes one)".into()),
        Err(e) => report(false, format!("config: {e}")),
    }

    match Gpu::new() {
        Ok(gpu) => {
            let info = gpu.info();
            let software = info.kind == "Cpu";
            report(!software, format!("graphics: {} ({}, {})", info.name, info.backend, info.driver));
            if software {
                println!("      a software renderer is in use; install a Vulkan driver for your graphics card");
            }
        }
        Err(e) => report(false, format!("graphics: {e}")),
    }
    if bokehcam_infer::gpu_available() {
        report(true, "models can run on the graphics card".into());
    } else {
        println!("note  models will run on the processor only (no graphics-card inference available)");
    }

    match list_cameras().first() {
        Some(camera) => report(true, format!("camera: {} ({})", camera.name, camera.path)),
        None => report(false, "camera: none found".into()),
    }
    match list_virtual_cameras().first() {
        Some(device) => report(true, format!("virtual camera: {} ({})", device.name, device.path)),
        None => {
            report(false, "virtual camera: no v4l2loopback device".into());
            println!("      load it for this session:  scripts/load-module.sh");
            println!("      or install it permanently: scripts/install.sh");
        }
    }

    let installed: Vec<String> =
        manifest::all().into_iter().filter(|m| m.installed_path().is_some()).map(|m| m.name).collect();
    if installed.is_empty() {
        report(false, "models: none downloaded (run `bokehcam models fetch`)".into());
    } else {
        report(true, format!("models: {}", installed.join(", ")));
    }

    if problems > 0 {
        bail!("{problems} problem(s) found");
    }
    println!("Everything looks good.");
    Ok(())
}

fn snapshot(config_path: &Path, file: &Path, warmup: u64) -> Result<()> {
    let config = Config::load_or_default(config_path)?;
    let size = (config.output.width, config.output.height);
    let deadline = Instant::now() + Duration::from_secs(30);
    let wait = |problem: Option<String>| -> Result<()> {
        if let Some(problem) = problem {
            bail!("{problem}");
        }
        if Instant::now() > deadline {
            bail!("no picture arrived within 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
    };
    let problem_of = |status: Status| match status.state {
        State::Problem(problem) => Some(problem),
        _ => None,
    };

    // Use the running bokehcam if there is one, otherwise run the pipeline here.
    let (preview, model): (Preview, String) = match running()? {
        Some(mut client) => {
            let start = client.preview(size.0, size.1, u64::MAX)?.map_or(0, |p| p.sequence);
            let first = loop {
                if let Some(preview) = client.preview(size.0, size.1, 0)? {
                    break preview.sequence;
                }
                wait(problem_of(client.status()?))?;
            };
            let target = first.max(start) + warmup;
            loop {
                if let Some(preview) = client.preview(size.0, size.1, target)? {
                    break (preview, client.status()?.model);
                }
                wait(problem_of(client.status()?))?;
            }
        }
        None => {
            let engine = Engine::start(config, None);
            engine.set_preview(Some(size));
            let preview = loop {
                if let Some(preview) = engine.preview(warmup.max(1) - 1) {
                    break preview;
                }
                wait(problem_of(engine.status()))?;
            };
            let model = engine.status().model;
            engine.stop();
            (preview, model)
        }
    };

    let output = std::fs::File::create(file).with_context(|| format!("cannot create {}", file.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(output), preview.width, preview.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&preview.rgba)?;
    println!("wrote {} ({}x{}; model: {model})", file.display(), preview.width, preview.height);
    Ok(())
}
