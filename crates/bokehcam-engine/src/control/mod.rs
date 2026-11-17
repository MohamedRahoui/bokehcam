//! The control channel: how other processes talk to a running engine.
//!
//! The settings window and the `bokehcam status/get/set` commands are
//! clients; whichever process runs the engine is the server.

use std::io;

mod unix;

pub use unix::{Client, ControlServer};

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
}
