//! The control channel: how other processes talk to a running engine.
//!
//! The settings window and the `bokehcam status/get/set` commands are
//! clients; whichever process runs the engine is the server.

use std::io;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;

#[cfg(not(unix))]
mod unsupported;
#[cfg(not(unix))]
use unsupported as platform;

pub use platform::{Client, ControlServer};

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("bokehcam is not running")]
    NotRunning,
    #[error("bokehcam is already running")]
    AlreadyRunning,
    #[error("control channel: {0}")]
    Io(#[from] io::Error),
    #[error("control channel: unexpected message ({0})")]
    Protocol(String),
    /// The engine refused the request; the text says why.
    #[error("{0}")]
    Refused(String),
    #[error(
        "other programs cannot reach a running bokehcam on this platform yet, \
         so the service and the settings window are not available"
    )]
    Unsupported,
}
