//! Placeholder backend for platforms without an implementation yet. It keeps
//! the same API as the Linux backend so every other crate compiles unchanged.

use std::time::Duration;

use bokehcam_core::config;

use crate::{CaptureFormat, ControlInfo, DeviceInfo, MediaError, Mode, Result};

pub fn list_cameras() -> Vec<DeviceInfo> {
    Vec::new()
}

pub fn list_virtual_cameras() -> Vec<DeviceInfo> {
    Vec::new()
}

pub struct Camera;

impl Camera {
    pub fn open(_config: &config::Camera) -> Result<Self> {
        Err(MediaError::Unsupported("camera capture"))
    }
    pub fn device(&self) -> &str {
        ""
    }
    pub fn format(&self) -> CaptureFormat {
        unreachable!()
    }
    pub fn next_packet(&mut self) -> Result<&[u8]> {
        Err(MediaError::Unsupported("camera capture"))
    }
    pub fn modes(_device: &str) -> Result<Vec<Mode>> {
        Err(MediaError::Unsupported("camera capture"))
    }
    pub fn controls(_device: &str) -> Result<Vec<ControlInfo>> {
        Err(MediaError::Unsupported("camera controls"))
    }
}

pub struct VirtualCamera;

impl VirtualCamera {
    pub fn open(_device: &str, _width: u32, _height: u32, _fps: u32) -> Result<Self> {
        Err(MediaError::Unsupported("the virtual camera"))
    }
    pub fn device(&self) -> &str {
        ""
    }
    pub fn frame_bytes(&self) -> usize {
        0
    }
    pub fn write(&self, _yuyv: &[u8]) -> Result<()> {
        Err(MediaError::Unsupported("the virtual camera"))
    }
    pub fn watch_consumers(&self) -> Result<()> {
        Err(MediaError::Unsupported("the virtual camera"))
    }
    pub fn wait_consumer_change(&self, _timeout: Duration) -> Option<bool> {
        None
    }
}
