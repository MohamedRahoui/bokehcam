//! The settings schema: every user-facing setting with its label, help text,
//! type and allowed range.
//!
//! This is the single description that drives validation, the commented
//! config file, and the settings window. Adding a setting means adding a
//! field in [`crate::config`] and one entry here; no front end needs editing.

/// What has to restart for a changed setting to take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Apply {
    /// Takes effect on the next frame.
    Live,
    /// The model is reloaded (a brief pause).
    ReloadModel,
    /// The physical camera is reopened (about a second of frozen picture).
    ReopenCamera,
    /// The virtual camera is reopened; apps using it may need to reselect it.
    ReopenOutput,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
    Float {
        min: f64,
        max: f64,
        step: f64,
    },
    /// One of a fixed set of `(value, label)` pairs.
    Choice(&'static [(&'static str, &'static str)]),
    /// A capture device path, or "auto". Front ends list the cameras found.
    Camera,
    /// A virtual camera device path, or "auto".
    VirtualCamera,
    /// A model name, or "auto". Front ends list the installed models.
    Model,
    /// Free text.
    Text,
}

#[derive(Debug, Clone, Copy)]
pub struct Setting {
    /// Dotted path in the config file, e.g. `"background.blur"`.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub apply: Apply,
}

#[derive(Debug, Clone, Copy)]
pub struct Group {
    /// The config table name, e.g. `"background"`.
    pub id: &'static str,
    pub title: &'static str,
    pub help: &'static str,
    pub settings: &'static [Setting],
}

const fn float(min: f64, max: f64, step: f64) -> Kind {
    Kind::Float { min, max, step }
}

const fn int(min: i64, max: i64, step: i64) -> Kind {
    Kind::Int { min, max, step }
}

macro_rules! setting {
    ($key:literal, $label:literal, $kind:expr, $apply:ident, $help:literal) => {
        Setting { key: $key, label: $label, help: $help, kind: $kind, apply: Apply::$apply }
    };
}

pub const GROUPS: &[Group] = &[
    Group {
        id: "camera",
        title: "Camera",
        help: "The physical camera and its capture mode.",
        settings: &[
            setting!(
                "camera.device",
                "Camera",
                Kind::Camera,
                ReopenCamera,
                "\"auto\" picks the first camera found. Otherwise a device path such as /dev/video0."
            ),
            setting!(
                "camera.width",
                "Capture width",
                int(160, 7680, 2),
                ReopenCamera,
                "Capturing above the output size and downscaling gives a sharper picture than letting the camera scale internally."
            ),
            setting!(
                "camera.height",
                "Capture height",
                int(120, 4320, 2),
                ReopenCamera,
                "If the camera does not offer this size, the closest mode is used."
            ),
            setting!(
                "camera.fps",
                "Frame rate",
                int(1, 120, 1),
                ReopenOutput,
                "Frames per second requested from the camera; the virtual camera runs at the same rate."
            ),
            setting!(
                "camera.format",
                "Capture format",
                Kind::Choice(&[("auto", "Automatic"), ("mjpeg", "Motion-JPEG"), ("yuyv", "YUYV (uncompressed)")]),
                ReopenCamera,
                "Motion-JPEG is needed for high resolutions on most webcams. \"auto\" prefers it when the requested size needs it."
            ),
        ],
    },
    Group {
        id: "output",
        title: "Virtual camera",
        help: "The camera other apps see.",
        settings: &[
            setting!(
                "output.device",
                "Device",
                Kind::VirtualCamera,
                ReopenOutput,
                "\"auto\" uses the first v4l2loopback device. Otherwise a device path such as /dev/video10."
            ),
            setting!("output.width", "Width", int(160, 3840, 2), ReopenOutput, "Width of the picture apps receive."),
            setting!("output.height", "Height", int(120, 2160, 2), ReopenOutput, "Height of the picture apps receive."),
            setting!(
                "output.on_demand",
                "Only use the camera when needed",
                Kind::Bool,
                ReopenOutput,
                "The physical camera (and its light) is only on while an app is using the virtual camera."
            ),
            setting!(
                "output.idle_timeout",
                "Keep camera on for",
                float(0.0, 600.0, 0.5),
                Live,
                "Seconds to keep the camera open after the last app stops using it."
            ),
        ],
    },
    Group {
        id: "framing",
        title: "Framing",
        help: "Zoom and position, taken from the full-resolution capture so moderate zoom stays sharp.",
        settings: &[
            setting!(
                "framing.zoom",
                "Zoom",
                float(1.0, 8.0, 0.01),
                Live,
                "1.0 shows the camera's whole field of view."
            ),
            setting!(
                "framing.center_x",
                "Horizontal position",
                float(0.0, 1.0, 0.005),
                Live,
                "Where the view is centred, from the left edge (0) to the right edge (1)."
            ),
            setting!(
                "framing.center_y",
                "Vertical position",
                float(0.0, 1.0, 0.005),
                Live,
                "Where the view is centred, from the top (0) to the bottom (1)."
            ),
            setting!("framing.mirror", "Mirror", Kind::Bool, Live, "Flip the picture horizontally."),
        ],
    },
    Group {
        id: "background",
        title: "Background",
        help: "What happens to everything that is not you.",
        settings: &[
            setting!(
                "background.mode",
                "Effect",
                Kind::Choice(&[("blur", "Blur"), ("none", "None")]),
                Live,
                "\"None\" switches person detection off entirely."
            ),
            setting!(
                "background.blur",
                "Blur strength",
                float(0.1, 10.0, 0.1),
                Live,
                "Blur radius as a percentage of the picture height. 1 is subtle, 3 is strong."
            ),
        ],
    },
    Group {
        id: "segmentation",
        title: "Person detection",
        help: "The model that separates you from the background.",
        settings: &[
            setting!(
                "segmentation.model",
                "Model",
                Kind::Model,
                ReloadModel,
                "\"auto\" picks the most accurate installed model this machine runs within the frame time."
            ),
            setting!(
                "segmentation.backend",
                "Run on",
                Kind::Choice(&[("auto", "Automatic"), ("gpu", "Graphics card"), ("cpu", "Processor")]),
                ReloadModel,
                "\"Automatic\" uses the graphics card when it works and is faster, otherwise the processor."
            ),
            setting!(
                "segmentation.threads",
                "Processor threads",
                int(1, 64, 1),
                ReloadModel,
                "Threads used when the model runs on the processor."
            ),
        ],
    },
    Group {
        id: "mask",
        title: "Outline",
        help: "How the outline around you is cleaned up.",
        settings: &[
            setting!(
                "mask.temporal_smoothing",
                "Stability",
                float(0.0, 0.95, 0.01),
                Live,
                "Higher values calm a shimmering outline. Smoothing is motion-adaptive, so real movement is still followed immediately."
            ),
            setting!(
                "mask.edge_radius",
                "Edge snapping range",
                float(0.2, 10.0, 0.1),
                Live,
                "How far the outline may move to snap onto real edges in the picture, as a percentage of the picture height."
            ),
            setting!(
                "mask.edge_eps",
                "Edge smoothness",
                float(0.00001, 0.1, 0.00001),
                Live,
                "Lower follows weaker edges (crisper, can get noisy); higher gives a smoother, softer outline."
            ),
            setting!(
                "mask.cutoff_low",
                "Background cutoff",
                float(0.0, 0.98, 0.01),
                Live,
                "Confidence below this is background. Lower it to keep more of you (ears, hair)."
            ),
            setting!(
                "mask.cutoff_high",
                "Person cutoff",
                float(0.02, 1.0, 0.01),
                Live,
                "Confidence above this is fully you. Must be above the background cutoff."
            ),
            setting!(
                "mask.grow",
                "Grow outline",
                int(-50, 50, 1),
                Live,
                "Grow (positive) or shrink (negative) the outline, in output pixels."
            ),
        ],
    },
    Group {
        id: "color",
        title: "Colour",
        help: "Applied to the final picture.",
        settings: &[
            setting!("color.brightness", "Brightness", float(-1.0, 1.0, 0.01), Live, "0 leaves the picture unchanged."),
            setting!("color.contrast", "Contrast", float(0.5, 2.0, 0.01), Live, "1 leaves the picture unchanged."),
            setting!(
                "color.saturation",
                "Saturation",
                float(0.0, 2.0, 0.01),
                Live,
                "0 is black and white, 1 is unchanged."
            ),
            setting!(
                "color.gamma",
                "Midtones",
                float(0.5, 2.0, 0.01),
                Live,
                "Above 1 brightens the midtones without clipping highlights; below 1 darkens them."
            ),
        ],
    },
];

/// Look up a setting by its dotted key.
pub fn find(key: &str) -> Option<&'static Setting> {
    GROUPS.iter().flat_map(|g| g.settings).find(|s| s.key == key)
}
