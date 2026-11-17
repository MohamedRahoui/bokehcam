//! What the engine reports to front ends.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// What the engine is doing right now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum State {
    /// Starting up (opening the graphics device, loading the model).
    #[default]
    Starting,
    /// Ready; the physical camera is off because nothing is using the virtual camera.
    Idle,
    /// The camera is on and frames are flowing.
    Streaming,
    /// Something is wrong; the engine keeps retrying. The text says what.
    Problem(String),
}

/// Average time spent in one step of the pipeline, per frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub ms: f32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    /// Frames delivered per second, averaged over the last second or so.
    pub fps: f32,
    pub stages: Vec<Stage>,
    /// e.g. "/dev/video0, 3840x2160 Mjpeg at 30 fps"
    pub camera: String,
    /// e.g. "/dev/video10, 1920x1080"
    pub output: String,
    /// e.g. "selfie-multiclass on the graphics card (6.3 ms)"
    pub model: String,
    pub graphics: String,
    /// Whether an app is reading the virtual camera right now.
    pub in_use: bool,
}

impl Status {
    /// Milliseconds of work per frame on the processing thread.
    pub fn frame_ms(&self) -> f32 {
        self.stages.iter().filter(|s| s.name != "decode").map(|s| s.ms).sum()
    }
}

/// A small copy of the finished picture, for a preview window.
#[derive(Debug, Clone, Default)]
pub struct Preview {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA.
    pub rgba: Vec<u8>,
    /// Increases with every new picture.
    pub sequence: u64,
}

/// Exponential moving average of per-stage times.
#[derive(Default)]
pub(crate) struct Timer {
    stages: Vec<Stage>,
    mark: Option<Instant>,
}

impl Timer {
    pub fn start(&mut self) {
        self.mark = Some(Instant::now());
    }

    pub fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        if let Some(mark) = self.mark.replace(now) {
            self.record(name, (now - mark).as_secs_f32() * 1000.0);
        }
    }

    pub fn record(&mut self, name: &'static str, ms: f32) {
        match self.stages.iter_mut().find(|s| s.name == name) {
            Some(stage) => stage.ms = stage.ms * 0.9 + ms * 0.1,
            None => self.stages.push(Stage { name: name.to_string(), ms }),
        }
    }

    pub fn stages(&self) -> Vec<Stage> {
        self.stages.clone()
    }

    pub fn clear(&mut self) {
        self.stages.clear();
        self.mark = None;
    }
}
