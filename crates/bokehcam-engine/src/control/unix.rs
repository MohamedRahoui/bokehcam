//! The control channel over a Unix socket
//! (`$XDG_RUNTIME_DIR/bokehcam.sock`), carrying length-prefixed JSON
//! messages.
//!
//! Every message is a little-endian `u32` length followed by that many
//! bytes. Requests and responses are JSON; a preview response is followed by
//! one more message holding the raw RGBA picture.

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bokehcam_core::Config;
use serde::{Deserialize, Serialize};

use super::ControlError;
use crate::{Engine, Preview, Status};

/// A preview request older than this no longer keeps the camera on.
const PREVIEW_INTEREST: Duration = Duration::from_millis(1500);
/// Upper bound for one message, to fail fast on a corrupt stream.
const MAX_MESSAGE: u32 = 64 << 20;

#[derive(Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum Request {
    Status,
    GetConfig,
    /// The whole configuration as TOML text.
    SetConfig {
        config: String,
    },
    /// The latest preview newer than `after`, scaled to this size.
    Preview {
        width: u32,
        height: u32,
        after: u64,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Response {
    Status(Status),
    Config(String),
    Done,
    Refused(String),
    /// `Some` is followed by one raw message with the RGBA bytes.
    Preview(Option<PreviewHeader>),
}

#[derive(Serialize, Deserialize)]
struct PreviewHeader {
    width: u32,
    height: u32,
    sequence: u64,
}

fn write_raw(stream: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(bytes)
}

fn read_raw(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length);
    if length > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut bytes = vec![0u8; length as usize];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_json<T: Serialize>(stream: &mut UnixStream, message: &T) -> io::Result<()> {
    write_raw(stream, &serde_json::to_vec(message).map_err(io::Error::other)?)
}

fn read_json<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> io::Result<T> {
    serde_json::from_slice(&read_raw(stream)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// --- server ---------------------------------------------------------------------------

/// Serves the control channel for one engine until dropped.
pub struct ControlServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Listen on `path`. Fails with [`ControlError::AlreadyRunning`] if
    /// another engine is serving there.
    pub fn start(engine: Arc<Engine>, path: &Path) -> Result<Self, ControlError> {
        if UnixStream::connect(path).is_ok() {
            return Err(ControlError::AlreadyRunning);
        }
        let _ = std::fs::remove_file(path); // a stale socket from a crashed run
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new().name("bokehcam-control".into()).spawn({
            let stop = stop.clone();
            move || accept_loop(&listener, &engine, &stop)
        })?;
        Ok(Self { path: path.to_path_buf(), stop, thread: Some(thread) })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn accept_loop(listener: &UnixListener, engine: &Arc<Engine>, stop: &Arc<AtomicBool>) {
    let last_preview_request: Arc<Mutex<Option<Instant>>> = Arc::default();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let (engine, stop, interest) = (engine.clone(), stop.clone(), last_preview_request.clone());
                let _ = thread::Builder::new().name("bokehcam-client".into()).spawn(move || {
                    if let Err(e) = serve_client(stream, &engine, &stop, &interest)
                        && e.kind() != io::ErrorKind::UnexpectedEof
                    {
                        tracing::debug!("control client: {e}");
                    }
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                tracing::warn!("control channel: {e}");
                thread::sleep(Duration::from_secs(1));
            }
        }
        // Nobody has asked for a preview lately: let the camera go idle again.
        let mut interest = last_preview_request.lock().unwrap_or_else(|p| p.into_inner());
        if interest.is_some_and(|t| t.elapsed() > PREVIEW_INTEREST) {
            *interest = None;
            engine.set_preview(None);
        }
    }
}

fn serve_client(
    mut stream: UnixStream,
    engine: &Engine,
    stop: &AtomicBool,
    preview_interest: &Mutex<Option<Instant>>,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    while !stop.load(Ordering::Relaxed) {
        let request: Request = match read_json(&mut stream) {
            Ok(request) => request,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e),
        };
        match request {
            Request::Status => write_json(&mut stream, &Response::Status(engine.status()))?,
            Request::GetConfig => write_json(&mut stream, &Response::Config(engine.config().to_toml()))?,
            Request::SetConfig { config } => {
                let response = match Config::from_toml(&config) {
                    Ok(config) => {
                        engine.set_config(config);
                        Response::Done
                    }
                    Err(e) => Response::Refused(e.to_string()),
                };
                write_json(&mut stream, &response)?;
            }
            Request::Preview { width, height, after } => {
                let size = (width.clamp(16, 3840), height.clamp(16, 2160));
                *preview_interest.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
                engine.set_preview(Some(size));
                match engine.preview(after).filter(|p| (p.width, p.height) == size) {
                    Some(preview) => {
                        let header =
                            PreviewHeader { width: preview.width, height: preview.height, sequence: preview.sequence };
                        write_json(&mut stream, &Response::Preview(Some(header)))?;
                        write_raw(&mut stream, &preview.rgba)?;
                    }
                    None => write_json(&mut stream, &Response::Preview(None))?,
                }
            }
        }
    }
    Ok(())
}

// --- client ---------------------------------------------------------------------------

/// A connection to a running engine.
pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self, ControlError> {
        let stream = UnixStream::connect(path).map_err(|_| ControlError::NotRunning)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self { stream })
    }

    fn ask(&mut self, request: &Request) -> Result<Response, ControlError> {
        write_json(&mut self.stream, request)?;
        Ok(read_json(&mut self.stream)?)
    }

    pub fn status(&mut self) -> Result<Status, ControlError> {
        match self.ask(&Request::Status)? {
            Response::Status(status) => Ok(status),
            _ => Err(ControlError::Protocol("expected a status".into())),
        }
    }

    pub fn config(&mut self) -> Result<Config, ControlError> {
        match self.ask(&Request::GetConfig)? {
            Response::Config(text) => Config::from_toml(&text).map_err(|e| ControlError::Protocol(e.to_string())),
            _ => Err(ControlError::Protocol("expected a config".into())),
        }
    }

    /// Apply a configuration. It is validated by the engine, takes effect
    /// within a fraction of a second, and is saved to the config file.
    pub fn set_config(&mut self, config: &Config) -> Result<(), ControlError> {
        match self.ask(&Request::SetConfig { config: config.to_toml() })? {
            Response::Done => Ok(()),
            Response::Refused(reason) => Err(ControlError::Refused(reason)),
            _ => Err(ControlError::Protocol("expected a confirmation".into())),
        }
    }

    /// The latest preview picture newer than `after`, if there is one yet.
    /// Asking regularly keeps the camera on.
    pub fn preview(&mut self, width: u32, height: u32, after: u64) -> Result<Option<Preview>, ControlError> {
        match self.ask(&Request::Preview { width, height, after })? {
            Response::Preview(None) => Ok(None),
            Response::Preview(Some(header)) => {
                let rgba = read_raw(&mut self.stream)?;
                if rgba.len() != (header.width * header.height * 4) as usize {
                    return Err(ControlError::Protocol("preview size mismatch".into()));
                }
                Ok(Some(Preview { width: header.width, height: header.height, rgba, sequence: header.sequence }))
            }
            _ => Err(ControlError::Protocol("expected a preview".into())),
        }
    }
}

#[cfg(test)]
mod tests;
