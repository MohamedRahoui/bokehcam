//! The control channel, between a real server and client over a socket in
//! the temporary directory, with the engine on fake devices.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bokehcam_core::Config;

use super::{Client, ControlError, ControlServer, MAX_MESSAGE, PREVIEW_INTEREST};
use crate::fakes::{FAST, Rig};
use crate::{Engine, State};

/// Nothing in these tests takes anywhere near this long unless it is broken.
const PATIENCE: Duration = Duration::from_secs(10);

/// A socket path in the temporary directory, removed when dropped.
struct SocketPath(PathBuf);

impl SocketPath {
    fn new(test: &str) -> Self {
        Self(std::env::temp_dir().join(format!("bokehcam-control-{}-{test}.sock", std::process::id())))
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// An idle engine on fake devices, served on a socket of its own.
struct Harness {
    rig: Arc<Rig>,
    engine: Arc<Engine>,
    // Dropped before the path, so the server removes its own socket first.
    server: Option<ControlServer>,
    socket: SocketPath,
}

fn config() -> Config {
    let mut config = Config::default();
    config.output.width = 160;
    config.output.height = 120;
    config.output.idle_timeout = 0.0;
    config
}

fn start(test: &str) -> Harness {
    let (rig, devices, stages) = Rig::new();
    let engine = Arc::new(Engine::start_with(config(), None, devices, stages, FAST));
    let socket = SocketPath::new(test);
    let server = ControlServer::start(engine.clone(), &socket.0).expect("the socket is free");
    let harness = Harness { rig, engine, server: Some(server), socket };
    wait_until("the engine is idle", || harness.engine.status().state == State::Idle);
    harness
}

impl Harness {
    fn client(&self) -> Client {
        Client::connect(&self.socket.0).expect("the server is listening")
    }

    /// A connection that speaks the protocol by hand.
    fn raw(&self) -> UnixStream {
        let stream = UnixStream::connect(&self.socket.0).expect("the server is listening");
        stream.set_read_timeout(Some(PATIENCE)).expect("a timeout can be set");
        stream
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "gave up waiting until {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// True when the server has closed this connection without answering.
fn closed_without_an_answer(mut stream: UnixStream) -> bool {
    let mut answer = Vec::new();
    matches!(stream.read_to_end(&mut answer), Ok(0))
}

// --- requests ---

#[test]
fn status_is_the_engines_status() {
    let harness = start("status");

    let status = harness.client().status().expect("a status");

    assert_eq!(status, harness.engine.status());
    assert_eq!(status.graphics, "fake graphics");
}

#[test]
fn the_config_arrives_as_the_engine_holds_it() {
    let harness = start("get-config");

    assert_eq!(harness.client().config().expect("a config"), config());
}

#[test]
fn a_config_sent_by_a_client_is_applied() {
    let harness = start("set-config");
    let mut client = harness.client();
    let mut changed = config();
    changed.framing.zoom = 2.5;

    client.set_config(&changed).expect("a valid config is accepted");

    wait_until("the engine holds the new config", || harness.engine.config() == changed);
    assert_eq!(client.config().expect("a config"), changed);
}

#[test]
fn an_invalid_config_is_refused_with_the_reason_and_changes_nothing() {
    let harness = start("refused");
    let mut client = harness.client();
    let mut invalid = config();
    invalid.framing.zoom = 100.0;

    let refusal = client.set_config(&invalid);

    let Err(ControlError::Refused(reason)) = refusal else { panic!("expected a refusal, got {refusal:?}") };
    assert!(reason.contains("zoom"), "the reason names the setting: {reason}");
    assert_eq!(client.config().expect("the connection still works"), config());
}

#[test]
fn one_connection_serves_many_requests_and_many_connections_are_served_at_once() {
    let harness = start("many");
    let (mut first, mut second) = (harness.client(), harness.client());

    for _ in 0..3 {
        first.status().expect("a status");
        second.config().expect("a config");
    }
}

// --- previews ---

#[test]
fn a_preview_has_the_requested_size_and_four_bytes_per_pixel() {
    let harness = start("preview");
    let mut client = harness.client();

    let mut preview = None;
    wait_until("a preview arrives", || {
        preview = client.preview(32, 18, 0).expect("a preview answer");
        preview.is_some()
    });

    let preview = preview.expect("just seen");
    assert_eq!((preview.width, preview.height), (32, 18));
    assert_eq!(preview.rgba.len(), 32 * 18 * 4);
}

#[test]
fn a_preview_already_seen_is_not_sent_again() {
    let harness = start("preview-newer");
    let mut client = harness.client();
    wait_until("a preview arrives", || client.preview(32, 18, 0).expect("a preview answer").is_some());

    let far_ahead = u64::MAX / 2;

    assert!(client.preview(32, 18, far_ahead).expect("a preview answer").is_none());
}

#[test]
fn asking_for_previews_keeps_the_camera_on_and_stopping_lets_it_go_off() {
    let harness = start("preview-interest");
    let mut client = harness.client();
    wait_until("a preview arrives", || client.preview(32, 18, 0).expect("a preview answer").is_some());
    assert!(harness.rig.camera_is_open());

    let last_asked = Instant::now();

    wait_until("the camera is closed", || !harness.rig.camera_is_open());
    assert!(last_asked.elapsed() >= PREVIEW_INTEREST, "closed after only {:?}", last_asked.elapsed());
}

// --- broken clients ---

#[test]
fn an_oversized_message_ends_that_connection_and_no_other() {
    let harness = start("oversized");
    let mut bystander = harness.client();
    let mut stream = harness.raw();

    stream.write_all(&(MAX_MESSAGE + 1).to_le_bytes()).expect("the length is sent");

    assert!(closed_without_an_answer(stream));
    bystander.status().expect("an existing connection still works");
    harness.client().status().expect("a new connection still works");
}

#[test]
fn a_message_cut_short_ends_that_connection_and_no_other() {
    let harness = start("truncated");
    let mut stream = harness.raw();

    stream.write_all(&50u32.to_le_bytes()).expect("the length is sent");
    stream.write_all(b"{\"req").expect("part of the message is sent");
    stream.shutdown(Shutdown::Write).expect("the sending side can be closed");

    assert!(closed_without_an_answer(stream));
    harness.client().status().expect("a new connection still works");
}

#[test]
fn a_message_that_is_not_a_request_ends_that_connection_and_no_other() {
    let harness = start("not-a-request");
    let mut stream = harness.raw();
    let message = b"{\"request\":\"format_the_disk\"}";

    stream.write_all(&(message.len() as u32).to_le_bytes()).expect("the length is sent");
    stream.write_all(message).expect("the message is sent");

    assert!(closed_without_an_answer(stream));
    harness.client().status().expect("a new connection still works");
}

// --- the socket ---

#[test]
fn a_second_server_on_the_same_socket_is_turned_away() {
    let harness = start("second-server");

    let second = ControlServer::start(harness.engine.clone(), &harness.socket.0);

    assert!(matches!(second, Err(ControlError::AlreadyRunning)));
    harness.client().status().expect("the first server is undisturbed");
}

#[test]
fn a_socket_left_behind_by_a_crashed_run_is_replaced() {
    let socket = SocketPath::new("stale");
    drop(UnixListener::bind(&socket.0).expect("the socket is free"));
    assert!(socket.0.exists(), "a dropped listener leaves its socket file behind");
    let (_rig, devices, stages) = Rig::new();
    let engine = Arc::new(Engine::start_with(config(), None, devices, stages, FAST));

    let _server = ControlServer::start(engine, &socket.0).expect("the stale socket is replaced");

    Client::connect(&socket.0).expect("the server is listening").status().expect("a status");
}

#[test]
fn connecting_where_nothing_listens_says_bokehcam_is_not_running() {
    let socket = SocketPath::new("nobody");

    assert!(matches!(Client::connect(&socket.0), Err(ControlError::NotRunning)));
}

#[test]
fn a_stopped_server_removes_its_socket() {
    let mut harness = start("stopped");

    harness.server = None;

    assert!(!harness.socket.0.exists());
    assert!(matches!(Client::connect(&harness.socket.0), Err(ControlError::NotRunning)));
}
