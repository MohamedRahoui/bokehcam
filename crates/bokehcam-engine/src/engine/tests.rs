//! The engine's behaviour, on fake devices and picture stages.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bokehcam_core::Config;

use super::{Engine, Timing};
use crate::State;
use crate::fakes::{OutputOpening, Rig};

/// Short enough that a test takes a fraction of a second.
const TIMING: Timing = Timing {
    retry: Duration::from_millis(30),
    frame_timeout: Duration::from_millis(500),
    config_poll: Duration::from_millis(10),
};
/// Seconds the camera stays on after the last reader left, in these tests.
const IDLE_TIMEOUT: f64 = 0.2;
/// Nothing in these tests takes anywhere near this long unless it is broken.
const PATIENCE: Duration = Duration::from_secs(10);
/// Long enough for the engine to act on anything it was going to act on.
const SETTLE: Duration = Duration::from_millis(300);

/// A small output and a short idle timeout; the camera only on demand.
fn config() -> Config {
    let mut config = Config::default();
    config.output.width = 160;
    config.output.height = 120;
    config.output.idle_timeout = IDLE_TIMEOUT;
    config
}

/// The same, with the camera on whether or not an app is reading.
fn always_on() -> Config {
    let mut config = config();
    config.output.on_demand = false;
    config
}

struct Harness {
    rig: Arc<Rig>,
    engine: Engine,
}

fn start(config: Config) -> Harness {
    start_with_file(config, None)
}

fn start_with_file(config: Config, config_path: Option<PathBuf>) -> Harness {
    let (rig, devices, stages) = Rig::new();
    Harness { rig, engine: Engine::start_with(config, config_path, devices, stages, TIMING) }
}

/// Start an engine whose rig was prepared before the worker ran.
fn start_prepared(config: Config, prepare: impl FnOnce(&Rig)) -> Harness {
    let (rig, devices, stages) = Rig::new();
    prepare(&rig);
    Harness { rig, engine: Engine::start_with(config, None, devices, stages, TIMING) }
}

impl Harness {
    fn state(&self) -> State {
        self.engine.status().state
    }

    fn wait_for_state(&self, state: State) {
        wait_until(&format!("the engine is {state:?}"), || self.state() == state);
    }

    /// Wait until frames written from now on have reached the virtual camera.
    fn wait_for_new_frames(&self) {
        let before = self.rig.frames_written();
        wait_until("new frames are written", || self.rig.frames_written() > before + 2);
    }

    /// An app starts reading once the engine is ready for it.
    fn start_reading(&self) {
        self.wait_for_state(State::Idle);
        self.rig.app_starts_reading();
        self.wait_for_state(State::Streaming);
    }
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "gave up waiting until {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// A config file in the temporary directory, removed when dropped.
struct ConfigFile(PathBuf);

impl ConfigFile {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bokehcam-engine-{}-{test}.toml", std::process::id()));
        Self(path)
    }
}

impl Drop for ConfigFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// --- the camera on demand ---

#[test]
fn the_camera_stays_off_until_an_app_reads_the_virtual_camera() {
    let harness = start(config());
    harness.wait_for_state(State::Idle);
    // The driver's first report, "nobody is reading", must not switch it on either.
    thread::sleep(SETTLE);
    assert_eq!(harness.rig.camera_opens(), 0);
    assert!(!harness.engine.status().in_use);

    harness.rig.app_starts_reading();

    harness.wait_for_state(State::Streaming);
    assert!(harness.rig.camera_is_open());
    assert!(harness.engine.status().in_use);
    harness.wait_for_new_frames();
    assert_eq!(harness.rig.last_frame_bytes(), 160 * 120 * 2);
}

#[test]
fn the_camera_closes_once_the_idle_timeout_has_passed() {
    let harness = start(config());
    harness.start_reading();

    let left = Instant::now();
    harness.rig.app_stops_reading();

    wait_until("the camera is closed", || !harness.rig.camera_is_open());
    assert!(left.elapsed() >= Duration::from_secs_f64(IDLE_TIMEOUT), "closed after only {:?}", left.elapsed());
    harness.wait_for_state(State::Idle);
}

#[test]
fn a_reader_that_returns_within_the_idle_timeout_keeps_the_camera_on() {
    let harness = start(config());
    harness.start_reading();

    harness.rig.app_stops_reading();
    thread::sleep(Duration::from_secs_f64(IDLE_TIMEOUT / 2.0));
    harness.rig.app_starts_reading();
    thread::sleep(Duration::from_secs_f64(IDLE_TIMEOUT * 2.0));

    assert!(harness.rig.camera_is_open());
    assert_eq!(harness.rig.camera_opens(), 1);
}

#[test]
fn without_on_demand_the_camera_is_on_with_no_reader() {
    let harness = start(always_on());

    harness.wait_for_state(State::Streaming);
    harness.wait_for_new_frames();
    assert!(!harness.engine.status().in_use);
}

#[test]
fn a_preview_request_turns_the_camera_on_and_its_end_turns_it_off() {
    let harness = start(config());
    harness.wait_for_state(State::Idle);

    harness.engine.set_preview(Some((32, 18)));

    wait_until("a preview arrives", || harness.engine.preview(0).is_some());
    let preview = harness.engine.preview(0).expect("just seen");
    assert_eq!((preview.width, preview.height), (32, 18));
    assert_eq!(preview.rgba.len(), 32 * 18 * 4);
    assert!(harness.engine.preview(preview.sequence + 1_000_000).is_none(), "only newer previews are returned");

    harness.engine.set_preview(None);

    wait_until("the camera is closed", || !harness.rig.camera_is_open());
}

// --- changed settings ---

#[test]
fn a_live_setting_reopens_nothing() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    let mut changed = harness.engine.config();
    changed.framing.zoom = 2.0;
    harness.engine.set_config(changed.clone());

    wait_until("the setting is in effect", || harness.engine.config() == changed);
    harness.wait_for_new_frames();
    assert_eq!(harness.rig.camera_opens(), 1);
    assert_eq!(harness.rig.output_openings().len(), 1);
}

#[test]
fn a_camera_setting_reopens_the_camera_but_not_the_virtual_camera() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    let mut changed = harness.engine.config();
    changed.camera.width = 1280;
    changed.camera.height = 720;
    harness.engine.set_config(changed);

    wait_until("the camera is opened again", || harness.rig.camera_opens() == 2);
    harness.wait_for_new_frames();
    assert_eq!(harness.rig.output_openings().len(), 1);
}

#[test]
fn an_output_setting_reopens_the_virtual_camera_at_the_new_size() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    let mut changed = harness.engine.config();
    changed.output.width = 320;
    changed.output.height = 180;
    harness.engine.set_config(changed);

    wait_until("the virtual camera is opened again", || harness.rig.output_openings().len() == 2);
    let fps = harness.engine.config().camera.fps;
    assert_eq!(harness.rig.output_openings()[1], OutputOpening { device: "auto".into(), width: 320, height: 180, fps });
    wait_until("frames of the new size are written", || harness.rig.last_frame_bytes() == 320 * 180 * 2);
}

#[test]
fn a_model_setting_reloads_the_model_without_reopening_the_camera() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);
    assert_eq!(harness.rig.models_loaded(), ["auto"]);

    let mut changed = harness.engine.config();
    changed.segmentation.model = "modnet".into();
    harness.engine.set_config(changed);

    wait_until("the other model is loaded", || harness.rig.models_loaded() == ["auto", "modnet"]);
    assert_eq!(harness.engine.status().model, "modnet on fake graphics");
    assert_eq!(harness.rig.camera_opens(), 1);
}

#[test]
fn a_setting_changed_by_a_front_end_is_written_to_the_config_file() {
    let file = ConfigFile::new("saved");
    let harness = start_with_file(config(), Some(file.0.clone()));
    harness.wait_for_state(State::Idle);

    let mut changed = harness.engine.config();
    changed.framing.zoom = 3.0;
    harness.engine.set_config(changed.clone());

    wait_until("the file holds the new settings", || Config::load(&file.0).is_ok_and(|saved| saved == changed));
}

#[test]
fn an_edited_config_file_is_applied() {
    let file = ConfigFile::new("edited");
    let harness = start_with_file(config(), Some(file.0.clone()));
    harness.wait_for_state(State::Idle);

    let mut edited = config();
    edited.framing.zoom = 4.0;
    edited.save(&file.0).expect("the temporary directory is writable");

    wait_until("the edit is in effect", || harness.engine.config() == edited);
}

#[test]
fn a_config_file_that_cannot_be_read_keeps_the_previous_settings() {
    let file = ConfigFile::new("invalid");
    let harness = start_with_file(config(), Some(file.0.clone()));
    harness.wait_for_state(State::Idle);

    std::fs::write(&file.0, "framing = [").expect("the temporary directory is writable");
    thread::sleep(SETTLE);

    assert_eq!(harness.engine.config(), config());
    assert_eq!(harness.state(), State::Idle);
}

// --- failures ---

#[test]
fn a_camera_that_cannot_be_opened_is_reported_and_tried_again() {
    let harness = start_prepared(always_on(), |rig| rig.break_camera("in use by another app"));

    wait_until(
        "the problem is reported",
        || matches!(harness.state(), State::Problem(problem) if problem.contains("in use by another app")),
    );
    assert_eq!(harness.rig.output_openings().len(), 1, "apps must be able to keep the virtual camera open");

    harness.rig.repair_camera();

    harness.wait_for_state(State::Streaming);
    harness.wait_for_new_frames();
}

#[test]
fn a_camera_that_stops_delivering_is_opened_again() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    harness.rig.fail_next_packet();

    wait_until("the camera is opened again", || harness.rig.camera_opens() == 2);
    harness.wait_for_state(State::Streaming);
    harness.wait_for_new_frames();
    assert_eq!(harness.rig.output_openings().len(), 1);
}

#[test]
fn a_few_corrupt_frames_are_skipped() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    harness.rig.corrupt_next_packets(5);

    wait_until("the corrupt frames have passed", || harness.rig.corrupt_packets_left() == 0);
    harness.wait_for_new_frames();
    assert_eq!(harness.state(), State::Streaming);
    assert_eq!(harness.rig.camera_opens(), 1);
}

#[test]
fn a_long_run_of_corrupt_frames_restarts_the_camera() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    harness.rig.corrupt_next_packets(40);

    wait_until("the camera is opened again", || harness.rig.camera_opens() == 2);
    harness.wait_for_state(State::Streaming);
    harness.wait_for_new_frames();
}

#[test]
fn a_model_that_cannot_be_loaded_is_reported_and_the_picture_still_delivered() {
    let harness = start_prepared(always_on(), Rig::break_models);

    wait_until(
        "the problem is reported",
        || matches!(harness.state(), State::Problem(problem) if problem.starts_with("background blur is off")),
    );
    harness.wait_for_new_frames();
    assert!(harness.engine.status().model.starts_with("unavailable"));
}

#[test]
fn no_model_is_loaded_when_the_blur_is_off() {
    let mut config = always_on();
    config.background.mode = "none".into();
    let harness = start(config);

    harness.wait_for_state(State::Streaming);
    harness.wait_for_new_frames();
    assert_eq!(harness.engine.status().model, "off");
    assert!(harness.rig.models_loaded().is_empty());
}

#[test]
fn missing_graphics_are_reported_and_tried_again() {
    let harness = start_prepared(config(), |rig| rig.break_graphics(1));

    wait_until("the problem is reported", || matches!(harness.state(), State::Problem(_)));

    harness.wait_for_state(State::Idle);
    assert_eq!(harness.engine.status().graphics, "fake graphics");
}

// --- stopping ---

#[test]
fn stopping_closes_the_camera_and_ends_the_worker() {
    let harness = start(always_on());
    harness.wait_for_state(State::Streaming);

    harness.engine.stop();

    assert!(!harness.rig.camera_is_open());
    assert!(!harness.engine.is_running());
}
