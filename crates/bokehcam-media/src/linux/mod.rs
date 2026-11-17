//! Linux backend: V4L2 capture and v4l2loopback output.

mod camera;
mod loopback;

pub use camera::{Camera, list_cameras};
pub use loopback::{VirtualCamera, list_virtual_cameras};

/// The driver name v4l2loopback reports.
const LOOPBACK_DRIVER: &str = "v4l2 loopback";
