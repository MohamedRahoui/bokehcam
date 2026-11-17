//! V4L2 camera capture.

use std::time::Duration;

use bokehcam_core::config;
use v4l::buffer::Type;
use v4l::capability::Flags as CapFlags;
use v4l::control::{Control, Flags as ControlFlags, Type as ControlType, Value};
use v4l::format::FourCC;
use v4l::framesize::FrameSizeEnum;
use v4l::io::traits::CaptureStream;
use v4l::prelude::*;
use v4l::video::Capture;
use v4l::video::capture::Parameters;

use super::LOOPBACK_DRIVER;
use crate::{CaptureFormat, ControlInfo, DeviceInfo, Encoding, MediaError, Mode, Result};

const MJPG: &[u8; 4] = b"MJPG";
const YUYV: &[u8; 4] = b"YUYV";
/// If no frame arrives for this long the camera is considered stalled.
const FRAME_TIMEOUT: Duration = Duration::from_secs(3);

fn open_error(device: &str) -> impl FnOnce(std::io::Error) -> MediaError + '_ {
    move |source| MediaError::Open { device: device.to_string(), source }
}

fn device_error(device: &str, problem: impl std::fmt::Display) -> MediaError {
    MediaError::Device { device: device.to_string(), problem: problem.to_string() }
}

/// Physical cameras: capture devices that offer at least one format and are
/// not virtual (loopback) devices.
pub fn list_cameras() -> Vec<DeviceInfo> {
    let mut found = Vec::new();
    let mut nodes = v4l::context::enum_devices();
    nodes.sort_by_key(|node| node.index());
    for node in nodes {
        let path = node.path().display().to_string();
        let Ok(device) = Device::with_path(node.path()) else { continue };
        let Ok(caps) = device.query_caps() else { continue };
        if caps.driver == LOOPBACK_DRIVER || !caps.capabilities.contains(CapFlags::VIDEO_CAPTURE) {
            continue;
        }
        // A camera exposes extra nodes (metadata) that have no video formats.
        if device.enum_formats().map(|f| f.is_empty()).unwrap_or(true) {
            continue;
        }
        found.push(DeviceInfo { path, name: caps.card });
    }
    found
}

fn resolve(device: &str) -> Result<String> {
    if device != "auto" {
        return Ok(device.to_string());
    }
    list_cameras().into_iter().next().map(|d| d.path).ok_or(MediaError::NoCamera)
}

/// An open, streaming camera.
pub struct Camera {
    device: String,
    format: CaptureFormat,
    stream: MmapStream<'static>,
}

impl Camera {
    pub fn open(config: &config::Camera) -> Result<Self> {
        let path = resolve(&config.device)?;
        let device = Device::with_path(&path).map_err(open_error(&path))?;

        let candidates: &[&[u8; 4]] = match config.format.as_str() {
            "mjpeg" => &[MJPG],
            "yuyv" => &[YUYV],
            _ => &[MJPG, YUYV],
        };
        let mut chosen = None;
        for fourcc in candidates {
            let mut wanted = device.format().map_err(open_error(&path))?;
            wanted.width = config.width;
            wanted.height = config.height;
            wanted.fourcc = FourCC::new(fourcc);
            match device.set_format(&wanted) {
                Ok(got) if got.fourcc == wanted.fourcc => {
                    chosen = Some(got);
                    break;
                }
                Ok(_) => continue,
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                    return Err(device_error(&path, "in use by another app"));
                }
                Err(_) => continue,
            }
        }
        let got = chosen.ok_or_else(|| device_error(&path, "offers neither Motion-JPEG nor YUYV"))?;
        let encoding = if &got.fourcc.repr == MJPG { Encoding::Mjpeg } else { Encoding::Yuyv };

        let fps = match device.set_params(&Parameters::with_fps(config.fps)) {
            Ok(params) if params.interval.numerator > 0 => {
                params.interval.denominator as f64 / params.interval.numerator as f64
            }
            _ => config.fps as f64,
        };
        apply_controls(&device, &path, &config.controls);

        let mut stream =
            MmapStream::with_buffers(&device, Type::VideoCapture, 4).map_err(|e| device_error(&path, e))?;
        stream.set_timeout(FRAME_TIMEOUT);

        let format = CaptureFormat { encoding, width: got.width, height: got.height, fps };
        if (got.width, got.height) != (config.width, config.height) {
            tracing::warn!(
                "camera gave {}x{} instead of the requested {}x{}",
                got.width,
                got.height,
                config.width,
                config.height
            );
        }
        tracing::info!("camera {path}: {}x{} {:?} @ {fps:.0} fps", got.width, got.height, encoding);
        Ok(Self { device: path, format, stream })
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn format(&self) -> CaptureFormat {
        self.format
    }

    /// Block until the next frame arrives and return its encoded bytes.
    pub fn next_packet(&mut self) -> Result<&[u8]> {
        let (buffer, meta) = self.stream.next().map_err(MediaError::Capture)?;
        let used = (meta.bytesused as usize).min(buffer.len());
        Ok(&buffer[..used])
    }

    /// The capture modes `device` offers ("auto" for the default camera).
    pub fn modes(device: &str) -> Result<Vec<Mode>> {
        let path = resolve(device)?;
        let device = Device::with_path(&path).map_err(open_error(&path))?;
        let mut modes = Vec::new();
        for (fourcc, encoding_mjpeg) in [(MJPG, true), (YUYV, false)] {
            let fourcc = FourCC::new(fourcc);
            for size in device.enum_framesizes(fourcc).unwrap_or_default() {
                let FrameSizeEnum::Discrete(size) = size.size else { continue };
                let max_fps = device
                    .enum_frameintervals(fourcc, size.width, size.height)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|i| match i.interval {
                        v4l::frameinterval::FrameIntervalEnum::Discrete(f) if f.numerator > 0 => {
                            Some(f.denominator / f.numerator)
                        }
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                modes.push(Mode { encoding_mjpeg, width: size.width, height: size.height, max_fps });
            }
        }
        modes.sort();
        Ok(modes)
    }

    /// The hardware controls of `device` ("auto" for the default camera).
    pub fn controls(device: &str) -> Result<Vec<ControlInfo>> {
        let path = resolve(device)?;
        let device = Device::with_path(&path).map_err(open_error(&path))?;
        let descriptions = device.query_controls().map_err(|e| device_error(&path, e))?;
        Ok(descriptions
            .into_iter()
            .filter(|d| matches!(d.typ, ControlType::Integer | ControlType::Boolean | ControlType::Menu))
            .map(|d| ControlInfo {
                name: control_name(&d.name),
                label: d.name.clone(),
                min: d.minimum,
                max: d.maximum,
                step: d.step,
                default: d.default,
                value: match device.control(d.id).map(|c| c.value) {
                    Ok(Value::Integer(v)) => Some(v),
                    Ok(Value::Boolean(v)) => Some(v as i64),
                    _ => None,
                },
                inactive: d.flags.contains(ControlFlags::INACTIVE),
            })
            .collect())
    }
}

/// The config-file name of a control: `"Zoom, Absolute"` becomes `zoom_absolute`
/// (the same convention as `v4l2-ctl`).
fn control_name(label: &str) -> String {
    let mut name = String::with_capacity(label.len());
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.ends_with('_') && !name.is_empty() {
            name.push('_');
        }
    }
    name.trim_end_matches('_').to_string()
}

/// Apply the configured hardware controls. Problems are logged, not fatal:
/// a camera without a "zoom" control should still work.
fn apply_controls(device: &Device, path: &str, wanted: &std::collections::BTreeMap<String, i64>) {
    if wanted.is_empty() {
        return;
    }
    let Ok(descriptions) = device.query_controls() else {
        tracing::warn!("{path}: cannot read hardware controls");
        return;
    };
    for (name, &value) in wanted {
        let Some(description) = descriptions.iter().find(|d| control_name(&d.name) == *name) else {
            tracing::warn!("{path}: no hardware control named `{name}`");
            continue;
        };
        let value = match description.typ {
            ControlType::Boolean => Value::Boolean(value != 0),
            _ => Value::Integer(value),
        };
        if let Err(e) = device.set_control(Control { id: description.id, value }) {
            tracing::warn!("{path}: setting `{name}` failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::control_name;

    #[test]
    fn control_names_follow_the_v4l2_ctl_convention() {
        assert_eq!(control_name("Zoom, Absolute"), "zoom_absolute");
        assert_eq!(control_name("White Balance, Automatic"), "white_balance_automatic");
        assert_eq!(control_name("Brightness"), "brightness");
        assert_eq!(control_name("Focus (absolute)"), "focus_absolute");
    }
}
