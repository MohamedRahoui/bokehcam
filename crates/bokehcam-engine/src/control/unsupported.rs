//! Placeholder for platforms where the control channel has no transport
//! yet. It keeps the names of the Unix implementation so the front ends
//! compile unchanged: no server can be started, so none is ever running.

use std::path::Path;
use std::sync::Arc;

use bokehcam_core::Config;

use super::ControlError;
use crate::{Engine, Preview, Status};

/// Cannot be started on this platform.
pub enum ControlServer {}

impl ControlServer {
    pub fn start(_engine: Arc<Engine>, _path: &Path) -> Result<Self, ControlError> {
        Err(ControlError::Unsupported)
    }
}

/// Cannot be connected on this platform: there is never a server.
pub enum Client {}

impl Client {
    pub fn connect(_path: &Path) -> Result<Self, ControlError> {
        Err(ControlError::NotRunning)
    }

    pub fn status(&mut self) -> Result<Status, ControlError> {
        match *self {}
    }

    pub fn config(&mut self) -> Result<Config, ControlError> {
        match *self {}
    }

    pub fn set_config(&mut self, _config: &Config) -> Result<(), ControlError> {
        match *self {}
    }

    pub fn preview(&mut self, _width: u32, _height: u32, _after: u64) -> Result<Option<Preview>, ControlError> {
        match *self {}
    }
}
