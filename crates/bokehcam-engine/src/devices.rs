//! The camera and the virtual camera as the engine sees them.
//!
//! The engine reaches its devices only through these traits, so its
//! behaviour can be exercised without a webcam or a v4l2loopback device.
//! On a real machine they are the types from `bokehcam-media`.

use std::time::Duration;

use bokehcam_core::config;
use bokehcam_media::{Camera, CaptureFormat, MediaError, VirtualCamera};

/// An opened camera delivering encoded frames.
pub(crate) trait FrameSource {
    /// What the camera was actually opened with.
    fn format(&self) -> CaptureFormat;

    /// Wait for the next frame, still encoded as [`Self::format`] says.
    fn next_packet(&mut self) -> Result<&[u8], MediaError>;
}

/// An opened virtual camera, fed with packed YUYV frames.
pub(crate) trait FrameSink: Sync {
    /// The device, as shown to the user.
    fn device(&self) -> &str;

    /// Hand one frame to the apps reading the virtual camera.
    fn write(&self, yuyv: &[u8]) -> Result<(), MediaError>;

    /// Start receiving "an app started or stopped reading" notifications.
    fn watch_consumers(&self) -> Result<(), MediaError>;

    /// Wait up to `timeout` for a change: `Some(true)` when an app is now
    /// reading, `Some(false)` when none is, `None` on timeout.
    fn wait_consumer_change(&self, timeout: Duration) -> Option<bool>;
}

/// Where the engine gets its devices from.
pub(crate) trait Devices: Send + Sync {
    fn open_camera(&self, config: &config::Camera) -> Result<Box<dyn FrameSource>, MediaError>;

    fn open_virtual_camera(
        &self,
        device: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Box<dyn FrameSink>, MediaError>;
}

/// The devices of this machine.
pub(crate) struct SystemDevices;

impl Devices for SystemDevices {
    fn open_camera(&self, config: &config::Camera) -> Result<Box<dyn FrameSource>, MediaError> {
        Ok(Box::new(Camera::open(config)?))
    }

    fn open_virtual_camera(
        &self,
        device: &str,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        Ok(Box::new(VirtualCamera::open(device, width, height, fps)?))
    }
}

impl FrameSource for Camera {
    fn format(&self) -> CaptureFormat {
        Camera::format(self)
    }

    fn next_packet(&mut self) -> Result<&[u8], MediaError> {
        Camera::next_packet(self)
    }
}

impl FrameSink for VirtualCamera {
    fn device(&self) -> &str {
        VirtualCamera::device(self)
    }

    fn write(&self, yuyv: &[u8]) -> Result<(), MediaError> {
        VirtualCamera::write(self, yuyv)
    }

    fn watch_consumers(&self) -> Result<(), MediaError> {
        VirtualCamera::watch_consumers(self)
    }

    fn wait_consumer_change(&self, timeout: Duration) -> Option<bool> {
        VirtualCamera::wait_consumer_change(self, timeout)
    }
}
