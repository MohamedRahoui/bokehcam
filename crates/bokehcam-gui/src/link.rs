//! The window's connection to the engine.
//!
//! The window never touches devices itself. It talks to whichever process
//! runs the engine over the control channel; if none is running, it starts
//! one in this process (and serves the control channel, so the command line
//! keeps working while the window is open).
//!
//! All blocking I/O happens on one background thread; the UI exchanges
//! messages with it through channels.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use bokehcam_core::{Config, paths};
use bokehcam_engine::control::{Client, ControlError, ControlServer};
use bokehcam_engine::{Engine, Preview, Status};

use crate::layout;

pub enum ToUi {
    Config(Config),
    Status(Status),
    Preview(Preview),
    /// A problem worth showing; `None` clears it.
    Problem(Option<String>),
}

pub enum ToLink {
    SetConfig(Box<Config>),
    /// Ask for previews of this size from now on.
    PreviewSize((u32, u32)),
}

pub struct Link {
    pub to_link: Sender<ToLink>,
    /// An engine running in this process, when no service was found.
    _local: Option<(ControlServer, Arc<Engine>)>,
}

impl Link {
    /// Connect to the running engine, or start one here. Returns the link,
    /// the stream of messages for the UI, and the current configuration.
    pub fn start(config_path: PathBuf) -> anyhow::Result<(Self, Receiver<ToUi>, Config)> {
        let socket = paths::control_socket();
        let (local, client) = match Client::connect(&socket) {
            Ok(client) => (None, client),
            Err(ControlError::NotRunning) => {
                let config = Config::load_or_default(&config_path)?;
                let engine = Arc::new(Engine::start(config, Some(config_path)));
                let server = ControlServer::start(engine.clone(), &socket)?;
                (Some((server, engine)), Client::connect(&socket)?)
            }
            Err(e) => return Err(e.into()),
        };
        let mut client = client;
        let config = client.config()?;

        let (to_ui, from_link) = mpsc::channel();
        let (to_link, from_ui) = mpsc::channel();
        let known = config.clone();
        // The window says how large it shows the picture once it has laid it out.
        let preview_size = layout::preview_request((config.output.width, config.output.height), 0.0);
        thread::Builder::new()
            .name("bokehcam-link".into())
            .spawn(move || run(client, known, preview_size, &to_ui, &from_ui))?;
        Ok((Self { to_link, _local: local }, from_link, config))
    }
}

fn run(
    mut client: Client,
    mut known: Config,
    mut preview_size: (u32, u32),
    to_ui: &Sender<ToUi>,
    from_ui: &Receiver<ToLink>,
) {
    let mut sequence = 0;
    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut last_config = Instant::now();
    loop {
        let result: Result<(), ControlError> = (|| {
            // Only the newest requested config matters.
            let mut wanted = None;
            loop {
                match from_ui.try_recv() {
                    Ok(ToLink::SetConfig(config)) => wanted = Some(*config),
                    Ok(ToLink::PreviewSize(size)) => preview_size = size,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return Err(ControlError::NotRunning),
                }
            }
            if let Some(config) = wanted {
                match client.set_config(&config) {
                    Ok(()) => {
                        known = config;
                        last_config = Instant::now();
                        let _ = to_ui.send(ToUi::Problem(None));
                    }
                    Err(ControlError::Refused(reason)) => {
                        let _ = to_ui.send(ToUi::Problem(Some(reason)));
                    }
                    Err(e) => return Err(e),
                }
            }
            if let Some(preview) = client.preview(preview_size.0, preview_size.1, sequence)? {
                sequence = preview.sequence;
                let _ = to_ui.send(ToUi::Preview(preview));
            }
            if last_status.elapsed() >= Duration::from_millis(500) {
                last_status = Instant::now();
                let _ = to_ui.send(ToUi::Status(client.status()?));
            }
            // Pick up changes made elsewhere (the config file, `bokehcam set`).
            if last_config.elapsed() >= Duration::from_secs(1) {
                last_config = Instant::now();
                let config = client.config()?;
                if config != known {
                    known = config.clone();
                    let _ = to_ui.send(ToUi::Config(config));
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => thread::sleep(Duration::from_millis(15)),
            // The window closed.
            Err(ControlError::NotRunning)
                if from_ui.try_recv().is_err_and(|e| e == mpsc::TryRecvError::Disconnected) =>
            {
                return;
            }
            Err(e) => {
                if to_ui.send(ToUi::Problem(Some(format!("lost contact with bokehcam: {e}")))).is_err() {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
                if let Ok(new) = Client::connect(&paths::control_socket()) {
                    client = new;
                    sequence = 0;
                    let _ = to_ui.send(ToUi::Problem(None));
                }
            }
        }
    }
}
