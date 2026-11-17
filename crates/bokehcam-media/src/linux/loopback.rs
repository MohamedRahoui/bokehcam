//! The producer side of a v4l2loopback device.
//!
//! Written directly against the kernel ABI (`videodev2.h`) and the
//! v4l2loopback private API, because we need its "client usage" event, which
//! tells a producer whether any app is currently reading the virtual camera.

use std::ffi::CString;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use v4l::prelude::*;

use super::LOOPBACK_DRIVER;
use crate::{DeviceInfo, MediaError, Result};

// --- kernel ABI ---------------------------------------------------------------

#[repr(C)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

/// The kernel union holds pointer-bearing members, hence the 8-byte alignment.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
union FormatUnion {
    pix: PixFormat,
    raw: [u8; 200],
}

#[repr(C)]
struct Format {
    type_: u32,
    fmt: FormatUnion,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct OutputParm {
    capability: u32,
    outputmode: u32,
    numerator: u32,
    denominator: u32,
    extendedmode: u32,
    writebuffers: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
union ParmUnion {
    output: OutputParm,
    raw: [u8; 200],
}

#[repr(C)]
struct StreamParm {
    type_: u32,
    parm: ParmUnion,
}

#[repr(C)]
struct EventSubscription {
    type_: u32,
    id: u32,
    flags: u32,
    reserved: [u32; 5],
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
union EventUnion {
    client_usage_count: u32,
    data: [u8; 64],
}

#[repr(C)]
struct Event {
    type_: u32,
    u: EventUnion,
    pending: u32,
    sequence: u32,
    tv_sec: libc::c_long,
    tv_nsec: libc::c_long,
    id: u32,
    reserved: [u32; 8],
}

const fn ioc(direction: u64, nr: u64, size: usize) -> libc::c_ulong {
    ((direction << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | nr) as libc::c_ulong
}
const WRITE: u64 = 1;
const READ: u64 = 2;

const VIDIOC_QUERYCAP: libc::c_ulong = ioc(READ, 0, size_of::<Capability>());
const VIDIOC_S_FMT: libc::c_ulong = ioc(READ | WRITE, 5, size_of::<Format>());
const VIDIOC_S_PARM: libc::c_ulong = ioc(READ | WRITE, 22, size_of::<StreamParm>());
const VIDIOC_DQEVENT: libc::c_ulong = ioc(READ, 89, size_of::<Event>());
const VIDIOC_SUBSCRIBE_EVENT: libc::c_ulong = ioc(WRITE, 90, size_of::<EventSubscription>());

const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_FIELD_NONE: u32 = 1;
const V4L2_COLORSPACE_SRGB: u32 = 8;
const V4L2_EVENT_SUB_FL_SEND_INITIAL: u32 = 1;
const V4L2_PIX_FMT_YUYV: u32 = u32::from_le_bytes(*b"YUYV");
/// `V4L2_EVENT_PRI_CLIENT_USAGE` from `v4l2loopback.h` (0.13 and newer).
const LOOPBACK_EVENT_CLIENT_USAGE: u32 = 0x0800_0000 + 0x08E0_0000 + 1;

// --- device discovery -----------------------------------------------------------

/// Every v4l2loopback device on this machine.
pub fn list_virtual_cameras() -> Vec<DeviceInfo> {
    let mut found = Vec::new();
    let mut nodes = v4l::context::enum_devices();
    nodes.sort_by_key(|node| node.index());
    for node in nodes {
        let Ok(device) = Device::with_path(node.path()) else { continue };
        let Ok(caps) = device.query_caps() else { continue };
        if caps.driver == LOOPBACK_DRIVER {
            found.push(DeviceInfo { path: node.path().display().to_string(), name: caps.card });
        }
    }
    found
}

// --- the device -------------------------------------------------------------------

/// An open v4l2loopback device, fed with packed YUYV frames.
pub struct VirtualCamera {
    device: String,
    fd: OwnedFd,
    frame_bytes: usize,
}

impl VirtualCamera {
    /// Open `device` ("auto" for the first loopback device) and announce the
    /// given format to apps.
    pub fn open(device: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        let path = if device == "auto" {
            list_virtual_cameras().into_iter().next().map(|d| d.path).ok_or(MediaError::NoVirtualCamera)?
        } else {
            device.to_string()
        };
        let fail = |problem: String| MediaError::Device { device: path.clone(), problem };
        let c_path = CString::new(path.clone()).map_err(|_| fail("invalid device path".into()))?;
        // SAFETY: plain open(2) on a NUL-terminated path; the result is checked.
        let raw = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(MediaError::Open { device: path, source: std::io::Error::last_os_error() });
        }
        // SAFETY: `raw` is a freshly opened descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        // SAFETY (all ioctls below): each passes a properly sized, initialised
        // `repr(C)` struct matching the request number's encoded size.
        let mut caps: Capability = unsafe { zeroed() };
        ioctl(&fd, VIDIOC_QUERYCAP, &mut caps).map_err(|e| fail(format!("not a video device ({e})")))?;
        let driver = String::from_utf8_lossy(&caps.driver);
        if driver.trim_end_matches('\0') != LOOPBACK_DRIVER {
            return Err(fail("not a v4l2loopback device".into()));
        }

        let frame_bytes = width as usize * height as usize * 2;
        let mut format: Format = unsafe { zeroed() };
        format.type_ = V4L2_BUF_TYPE_VIDEO_OUTPUT;
        format.fmt.pix = PixFormat {
            width,
            height,
            pixelformat: V4L2_PIX_FMT_YUYV,
            field: V4L2_FIELD_NONE,
            bytesperline: width * 2,
            sizeimage: frame_bytes as u32,
            colorspace: V4L2_COLORSPACE_SRGB,
            priv_: 0,
            flags: 0,
            ycbcr_enc: 0,
            quantization: 0,
            xfer_func: 0,
        };
        ioctl(&fd, VIDIOC_S_FMT, &mut format).map_err(|e| {
            fail(format!(
                "cannot set the format ({e}); another program may be feeding it, or an app \
                 still has it open from before"
            ))
        })?;

        let mut parm: StreamParm = unsafe { zeroed() };
        parm.type_ = V4L2_BUF_TYPE_VIDEO_OUTPUT;
        parm.parm.output.numerator = 1;
        parm.parm.output.denominator = fps.max(1);
        ioctl(&fd, VIDIOC_S_PARM, &mut parm).map_err(|e| fail(format!("cannot set the frame rate ({e})")))?;

        let camera = Self { device: path, fd, frame_bytes };
        // With exclusive_caps=1 the device only behaves like a camera for other
        // apps once a producer is streaming, which starts at the first write.
        // One black frame makes the virtual camera usable (and able to wake us
        // up) while the physical camera is still off.
        camera.write(&black_yuyv(width, height))?;
        Ok(camera)
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    /// Size of one frame in bytes (`width * height * 2`).
    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Hand one YUYV frame to the apps reading the virtual camera.
    pub fn write(&self, yuyv: &[u8]) -> Result<()> {
        debug_assert_eq!(yuyv.len(), self.frame_bytes);
        // SAFETY: the pointer and length describe the caller's slice.
        let written = unsafe { libc::write(self.fd.as_raw_fd(), yuyv.as_ptr().cast(), yuyv.len()) };
        if written < 0 {
            let problem = format!("write failed: {}", std::io::Error::last_os_error());
            return Err(MediaError::Device { device: self.device.clone(), problem });
        }
        Ok(())
    }

    /// Start receiving "an app started/stopped reading" notifications.
    pub fn watch_consumers(&self) -> Result<()> {
        let mut subscription: EventSubscription = unsafe { zeroed() };
        subscription.type_ = LOOPBACK_EVENT_CLIENT_USAGE;
        subscription.flags = V4L2_EVENT_SUB_FL_SEND_INITIAL;
        ioctl(&self.fd, VIDIOC_SUBSCRIBE_EVENT, &mut subscription).map_err(|e| MediaError::Device {
            device: self.device.clone(),
            problem: format!("cannot tell whether apps are using it ({e}); v4l2loopback 0.13 or newer is needed"),
        })
    }

    /// Wait up to `timeout` for a change. `Some(true)` when an app is now
    /// reading the virtual camera, `Some(false)` when none is, `None` on timeout.
    pub fn wait_consumer_change(&self, timeout: Duration) -> Option<bool> {
        let mut poll = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLPRI, revents: 0 };
        // SAFETY: `poll` points to one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
        if ready <= 0 {
            return None;
        }
        let mut state = None;
        loop {
            // Drain the queue; only the latest state matters.
            let mut event: Event = unsafe { zeroed() };
            if ioctl(&self.fd, VIDIOC_DQEVENT, &mut event).is_err() {
                break;
            }
            if event.type_ == LOOPBACK_EVENT_CLIENT_USAGE {
                // SAFETY: for this event type the driver fills `client_usage_count`.
                state = Some(unsafe { event.u.client_usage_count } > 0);
            }
            if event.pending == 0 {
                break;
            }
        }
        state
    }
}

fn ioctl<T>(fd: &OwnedFd, request: libc::c_ulong, argument: &mut T) -> std::io::Result<()> {
    // SAFETY: callers pass the struct type that `request` was computed from.
    let result = unsafe { libc::ioctl(fd.as_raw_fd(), request, argument as *mut T) };
    if result < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// One black frame in packed YUYV (limited range: Y=16, U=V=128).
fn black_yuyv(width: u32, height: u32) -> Vec<u8> {
    [16u8, 128].repeat(width as usize * height as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structs must match the kernel ABI exactly (x86-64 and aarch64).
    #[test]
    fn struct_sizes_and_ioctl_numbers_match_videodev2_h() {
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(size_of::<PixFormat>(), 48);
        assert_eq!(size_of::<Format>(), 208);
        assert_eq!(size_of::<StreamParm>(), 204);
        assert_eq!(size_of::<EventSubscription>(), 32);
        assert_eq!(size_of::<Event>(), 136);
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_S_FMT, 0xC0D0_5605);
        assert_eq!(VIDIOC_S_PARM, 0xC0CC_5616);
        assert_eq!(VIDIOC_DQEVENT, 0x8088_5659);
        assert_eq!(VIDIOC_SUBSCRIBE_EVENT, 0x4020_565A);
        assert_eq!(LOOPBACK_EVENT_CLIENT_USAGE, 0x10E0_0001);
    }

    #[test]
    fn black_frame() {
        let frame = black_yuyv(4, 2);
        assert_eq!(frame.len(), 16);
        assert_eq!(&frame[..4], &[16, 128, 16, 128]);
    }
}
