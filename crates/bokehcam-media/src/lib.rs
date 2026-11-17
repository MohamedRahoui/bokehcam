//! Camera capture, frame decoding and the virtual camera device.
//!
//! The public API is platform-neutral; the implementation currently exists
//! for Linux (V4L2 capture, v4l2loopback output). Other platforms compile
//! and report [`MediaError::Unsupported`], so the rest of the workspace
//! builds everywhere and a new backend only has to fill in this crate.

pub mod decode;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
use unsupported as platform;

pub use decode::Decoder;
pub use platform::{Camera, VirtualCamera, list_cameras, list_virtual_cameras};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("no camera found (connect one, or set camera.device)")]
    NoCamera,
    #[error(
        "no virtual camera device found: the v4l2loopback kernel module is not loaded \
         (see `bokehcam doctor`)"
    )]
    NoVirtualCamera,
    #[error("cannot open {device}: {source}")]
    Open { device: String, source: std::io::Error },
    #[error("{device}: {problem}")]
    Device { device: String, problem: String },
    #[error("camera stopped delivering frames: {0}")]
    Capture(std::io::Error),
    #[error("cannot decode frame: {0}")]
    Decode(String),
    #[error("{0} is not supported on this platform yet")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, MediaError>;

/// How the camera encodes its frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Mjpeg,
    Yuyv,
}

/// A camera or virtual camera present on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub path: String,
    pub name: String,
}

/// A capture mode a camera offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mode {
    pub encoding_mjpeg: bool,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
}

/// A hardware control of a camera (zoom, focus, exposure ...).
#[derive(Debug, Clone, PartialEq)]
pub struct ControlInfo {
    /// Stable name used in the config file, e.g. `zoom_absolute`.
    pub name: String,
    pub label: String,
    pub min: i64,
    pub max: i64,
    pub step: u64,
    pub default: i64,
    pub value: Option<i64>,
    /// Currently locked by another control (e.g. manual focus while autofocus is on).
    pub inactive: bool,
}

/// What a camera was actually opened with (drivers may adjust the request).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureFormat {
    pub encoding: Encoding,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

/// A decoded frame: tightly packed 8-bit RGBA, top row first.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
