//! The stages between a decoded camera frame and the frame apps receive.
//!
//! The engine drives them through [`Picture`], so its own behaviour can be
//! exercised without a graphics device or a model. On a real machine they
//! are the renderer from `bokehcam-gpu` and the model from `bokehcam-infer`.

use bokehcam_gpu::{FrameParams, Gpu, Renderer, Sizes};
use bokehcam_infer::{Choice, Segmenter};
use bokehcam_media::Frame;

use crate::EngineError;
use crate::status::Timer;

/// The person-detection model the configuration asks for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelRequest {
    /// A model name, or "auto".
    pub model: String,
    /// "auto", "gpu" or "cpu".
    pub backend: String,
    /// Threads to use when the model runs on the processor.
    pub threads: u32,
    /// With "auto", the model has to run within this many milliseconds.
    pub budget_ms: f32,
}

/// Everything that turns a camera frame into an output frame.
pub(crate) trait Picture {
    /// What the picture is computed on, as shown to the user.
    fn graphics(&self) -> String;

    /// Load the requested model, replacing any loaded one. Returns a
    /// description of what was loaded, as shown to the user. When it fails,
    /// no model is loaded.
    fn load_model(&mut self, request: &ModelRequest) -> Result<String, EngineError>;

    /// Stop detecting the person.
    fn drop_model(&mut self);

    /// Turn `frame` into a packed YUYV frame of the `output` size, recording
    /// how long each stage took.
    fn render(
        &mut self,
        frame: &Frame,
        output: (u32, u32),
        params: &FrameParams,
        timer: &mut Timer,
    ) -> Result<&[u8], EngineError>;

    /// The frame rendered last, scaled to `width` x `height`, as RGBA. Only
    /// to be called after a [`Self::render`] since the last [`Self::rest`].
    fn preview(&mut self, width: u32, height: u32) -> Result<&[u8], EngineError>;

    /// The camera stopped: let go of what only a running stream needs.
    fn rest(&mut self);
}

/// Where the engine gets its picture stages from.
pub(crate) trait PictureStages: Send + Sync {
    fn open(&self) -> Result<Box<dyn Picture>, EngineError>;
}

/// The picture stages of this machine: its graphics device and the models
/// installed on it.
pub(crate) struct SystemPictureStages;

impl PictureStages for SystemPictureStages {
    fn open(&self) -> Result<Box<dyn Picture>, EngineError> {
        Ok(Box::new(GpuPicture { gpu: Gpu::new()?, renderer: None, segmenter: None, mask: Vec::new() }))
    }
}

struct GpuPicture {
    gpu: Gpu,
    /// Built for the sizes of the stream in progress.
    renderer: Option<Renderer>,
    segmenter: Option<Segmenter>,
    /// Person probabilities from the last model run, reused between frames.
    mask: Vec<u8>,
}

impl Picture for GpuPicture {
    fn graphics(&self) -> String {
        format!("{} ({})", self.gpu.info().name, self.gpu.info().backend)
    }

    fn load_model(&mut self, request: &ModelRequest) -> Result<String, EngineError> {
        // The old model goes first, so two are never in memory together.
        self.segmenter = None;
        let segmenter = Segmenter::load(
            &request.model,
            Choice::parse(&request.backend),
            request.threads as usize,
            request.budget_ms,
        )?;
        let description = format!(
            "{} on {} ({:.1} ms)",
            segmenter.manifest().name,
            segmenter.backend().label(),
            segmenter.typical_ms()
        );
        self.segmenter = Some(segmenter);
        Ok(description)
    }

    fn drop_model(&mut self) {
        self.segmenter = None;
    }

    fn render(
        &mut self,
        frame: &Frame,
        output: (u32, u32),
        params: &FrameParams,
        timer: &mut Timer,
    ) -> Result<&[u8], EngineError> {
        let gpu = &self.gpu;
        let sizes = Sizes {
            source: (frame.width, frame.height),
            output,
            model: self.segmenter.as_ref().map(Segmenter::input_size),
        };
        if self.renderer.as_ref().map(Renderer::sizes) != Some(sizes) {
            self.renderer = Some(Renderer::new(gpu, sizes));
        }
        let renderer = self.renderer.as_mut().expect("just created");

        renderer.upload(gpu, &frame.rgba);
        timer.lap("upload");
        let model_input = renderer.prepare(gpu, params)?;
        timer.lap("frame");
        if let (Some(image), Some(segmenter)) = (model_input, self.segmenter.as_mut()) {
            segmenter.run(image, &mut self.mask)?;
            renderer.set_mask(gpu, &self.mask);
            timer.lap("model");
        }
        let yuyv = renderer.finish(gpu, params)?;
        timer.lap("render");
        Ok(yuyv)
    }

    fn preview(&mut self, width: u32, height: u32) -> Result<&[u8], EngineError> {
        let renderer = self.renderer.as_mut().expect("a preview is only asked for after a render");
        Ok(renderer.preview(&self.gpu, width, height)?)
    }

    fn rest(&mut self) {
        self.renderer = None;
    }
}
