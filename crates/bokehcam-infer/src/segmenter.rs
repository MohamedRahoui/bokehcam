//! Person segmentation: an RGBA image in, a person-probability mask out.

use std::sync::OnceLock;
use std::time::Instant;

use ort::environment::Environment;
use ort::ep::{ExecutionProvider, WebGPU};
use ort::session::Session;
use ort::value::TensorRef;

use crate::InferError;
use crate::manifest::{self, Layout, Manifest, OutputKind, Range};

/// Where a model runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Gpu,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Cpu => "the processor",
            Backend::Gpu => "the graphics card",
        }
    }
}

/// What the user asked for (`segmentation.backend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    Cpu,
    Gpu,
}

impl Choice {
    pub fn parse(text: &str) -> Self {
        match text {
            "cpu" => Choice::Cpu,
            "gpu" => Choice::Gpu,
            _ => Choice::Auto,
        }
    }
}

fn environment() -> Result<&'static Environment, String> {
    static ENVIRONMENT: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| ort::init().with_name("bokehcam").build().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

/// Whether this build and machine can run models on the graphics card.
pub fn gpu_available() -> bool {
    environment().is_ok() && WebGPU::is_available().unwrap_or(false)
}

/// A loaded model, ready to run.
pub struct Segmenter {
    manifest: Manifest,
    backend: Backend,
    session: Session,
    input: Vec<f32>,
    input_name: String,
    /// Typical time for one run, measured when the model was loaded.
    typical_ms: f32,
}

impl Segmenter {
    /// Load the configured model on the configured backend.
    ///
    /// With `model = "auto"`, installed models are tried from most to least
    /// accurate and the first one that runs within `budget_ms` is used (the
    /// fastest one if none does). With `backend = Auto`, each model is
    /// measured on both backends and the faster one wins.
    pub fn load(model: &str, choice: Choice, threads: usize, budget_ms: f32) -> Result<Self, InferError> {
        if model != "auto" {
            let manifest = manifest::find(model)?;
            return Self::load_best_backend(&manifest, choice, threads);
        }
        let installed: Vec<Manifest> = manifest::all().into_iter().filter(|m| m.installed_path().is_some()).collect();
        if installed.is_empty() {
            return Err(InferError::NothingInstalled);
        }
        let mut fastest: Option<Segmenter> = None;
        for manifest in &installed {
            match Self::load_best_backend(manifest, choice, threads) {
                Ok(candidate) if candidate.typical_ms <= budget_ms => return Ok(candidate),
                Ok(candidate) => {
                    tracing::info!(
                        "model {} takes {:.1} ms here, over the {budget_ms:.0} ms budget; trying a lighter one",
                        manifest.name,
                        candidate.typical_ms
                    );
                    if fastest.as_ref().is_none_or(|f| candidate.typical_ms < f.typical_ms) {
                        fastest = Some(candidate);
                    }
                }
                Err(e) => tracing::warn!("{e}"),
            }
        }
        fastest.ok_or(InferError::NothingInstalled)
    }

    fn load_best_backend(manifest: &Manifest, choice: Choice, threads: usize) -> Result<Self, InferError> {
        let backends: &[Backend] = match choice {
            Choice::Cpu => &[Backend::Cpu],
            Choice::Gpu => &[Backend::Gpu],
            Choice::Auto if gpu_available() => &[Backend::Gpu, Backend::Cpu],
            Choice::Auto => &[Backend::Cpu],
        };
        let mut best: Option<Segmenter> = None;
        let mut last_error = None;
        for &backend in backends {
            match Self::load_on(manifest, backend, threads) {
                Ok(candidate) => {
                    tracing::debug!("{} on {}: {:.1} ms", manifest.name, backend.label(), candidate.typical_ms);
                    if best.as_ref().is_none_or(|b| candidate.typical_ms < b.typical_ms) {
                        best = Some(candidate);
                    }
                }
                Err(e) => {
                    tracing::debug!("{e}");
                    last_error = Some(e);
                }
            }
        }
        best.ok_or_else(|| last_error.expect("at least one backend was tried"))
    }

    /// Load `manifest` on one specific backend and measure it.
    pub fn load_on(manifest: &Manifest, backend: Backend, threads: usize) -> Result<Self, InferError> {
        let fail =
            |problem: String| InferError::Runtime { model: manifest.name.clone(), backend: backend.label(), problem };
        let path = manifest.installed_path().ok_or_else(|| InferError::NotInstalled(manifest.name.clone()))?;
        let environment = environment().map_err(&fail)?;
        let builder = Session::builder(environment).map_err(|e| fail(e.to_string()))?;
        let mut builder = match backend {
            // Threads sleep between frames instead of busy-waiting: at video
            // frame rates spinning burns whole cores for no gain.
            Backend::Cpu => builder
                .with_intra_threads(threads.max(1))
                .and_then(|b| b.with_intra_op_spinning(false))
                .and_then(|b| b.with_inter_op_spinning(false)),
            Backend::Gpu => builder.with_execution_providers([WebGPU::default().build().error_on_failure()]),
        }
        .map_err(|e| fail(e.to_string()))?;
        let session = builder.commit_from_file(&path).map_err(|e| fail(e.to_string()))?;
        let input_name =
            session.inputs().first().map(|i| i.name().to_string()).ok_or_else(|| fail("model has no input".into()))?;
        let (width, height) = (manifest.input.width as usize, manifest.input.height as usize);
        let mut segmenter = Self {
            manifest: manifest.clone(),
            backend,
            session,
            input: vec![0.0; width * height * 3],
            input_name,
            typical_ms: 0.0,
        };

        // Warm up (the first runs compile shaders / allocate), then measure.
        let image = vec![128u8; width * height * 4];
        let mut mask = Vec::new();
        for _ in 0..3 {
            segmenter.run(&image, &mut mask)?;
        }
        let mut times: Vec<f32> = (0..7)
            .map(|_| {
                let start = Instant::now();
                segmenter.run(&image, &mut mask).map(|()| start.elapsed().as_secs_f32() * 1000.0)
            })
            .collect::<Result<_, _>>()?;
        times.sort_by(f32::total_cmp);
        segmenter.typical_ms = times[times.len() / 2];
        Ok(segmenter)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn typical_ms(&self) -> f32 {
        self.typical_ms
    }

    /// The image size [`Self::run`] expects.
    pub fn input_size(&self) -> (u32, u32) {
        (self.manifest.input.width, self.manifest.input.height)
    }

    /// Run the model on a tightly packed RGBA image of [`Self::input_size`].
    /// `mask` receives one byte per pixel: 0 = background, 255 = person.
    pub fn run(&mut self, rgba: &[u8], mask: &mut Vec<u8>) -> Result<(), InferError> {
        let (width, height) = (self.manifest.input.width as usize, self.manifest.input.height as usize);
        let pixels = width * height;
        debug_assert_eq!(rgba.len(), pixels * 4);
        fill_input(rgba, &mut self.input, pixels, self.manifest.input.layout, self.manifest.input.range);
        let shape = match self.manifest.input.layout {
            Layout::Nhwc => [1, height, width, 3],
            Layout::Nchw => [1, 3, height, width],
        };

        let model = &self.manifest;
        let backend = self.backend;
        let fail =
            |problem: String| InferError::Runtime { model: model.name.clone(), backend: backend.label(), problem };
        let tensor = TensorRef::from_array_view((shape, self.input.as_slice())).map_err(|e| fail(e.to_string()))?;
        let outputs =
            self.session.run(ort::inputs![self.input_name.as_str() => tensor]).map_err(|e| fail(e.to_string()))?;
        let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(|e| fail(e.to_string()))?;
        if values.len() % pixels != 0 || values.is_empty() {
            return Err(fail(format!("unexpected output shape {shape:?}")));
        }
        mask.resize(pixels, 0);
        read_output(values, mask, pixels, model.output.kind, model.output.layout);
        Ok(())
    }
}

/// RGBA bytes to the model's float tensor.
fn fill_input(rgba: &[u8], input: &mut [f32], pixels: usize, layout: Layout, range: Range) {
    let (scale, offset) = match range {
        Range::ZeroToOne => (1.0 / 255.0, 0.0),
        Range::MinusOneToOne => (2.0 / 255.0, -1.0),
    };
    match layout {
        Layout::Nhwc => {
            for (px, out) in rgba.chunks_exact(4).zip(input.chunks_exact_mut(3)) {
                for c in 0..3 {
                    out[c] = px[c] as f32 * scale + offset;
                }
            }
        }
        Layout::Nchw => {
            let (r, rest) = input.split_at_mut(pixels);
            let (g, b) = rest.split_at_mut(pixels);
            for (i, px) in rgba.chunks_exact(4).enumerate() {
                r[i] = px[0] as f32 * scale + offset;
                g[i] = px[1] as f32 * scale + offset;
                b[i] = px[2] as f32 * scale + offset;
            }
        }
    }
}

/// The model's output to person probability, 0..255 per pixel.
fn read_output(values: &[f32], mask: &mut [u8], pixels: usize, kind: OutputKind, layout: Layout) {
    let channels = values.len() / pixels;
    let at = |pixel: usize, channel: usize| match layout {
        Layout::Nhwc => values[pixel * channels + channel],
        Layout::Nchw => values[channel * pixels + pixel],
    };
    for (pixel, out) in mask.iter_mut().enumerate() {
        let probability = match kind {
            OutputKind::Alpha => at(pixel, 0),
            // Two classes: softmax reduces to a sigmoid of the score difference.
            OutputKind::Logits if channels == 2 => 1.0 / (1.0 + (at(pixel, 0) - at(pixel, 1)).clamp(-30.0, 30.0).exp()),
            OutputKind::Logits => {
                let highest = (0..channels).map(|c| at(pixel, c)).fold(f32::MIN, f32::max);
                let total: f32 = (0..channels).map(|c| (at(pixel, c) - highest).exp()).sum();
                1.0 - (at(pixel, 0) - highest).exp() / total
            }
        };
        *out = (probability.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_layouts_and_ranges() {
        let rgba = [0, 128, 255, 9, 255, 0, 0, 9];
        let mut nhwc = [0.0; 6];
        fill_input(&rgba, &mut nhwc, 2, Layout::Nhwc, Range::ZeroToOne);
        assert_eq!(nhwc[0], 0.0);
        assert!((nhwc[1] - 0.50196).abs() < 1e-4);
        assert_eq!((nhwc[2], nhwc[3]), (1.0, 1.0));

        let mut nchw = [0.0; 6];
        fill_input(&rgba, &mut nchw, 2, Layout::Nchw, Range::MinusOneToOne);
        // Planes: R R, G G, B B.
        assert_eq!((nchw[0], nchw[1]), (-1.0, 1.0));
        assert_eq!((nchw[4], nchw[5]), (1.0, -1.0));
    }

    #[test]
    fn two_class_logits_become_a_sigmoid() {
        let values = [4.0, -4.0, -4.0, 4.0, 0.0, 0.0];
        let mut mask = [0u8; 3];
        read_output(&values, &mut mask, 3, OutputKind::Logits, Layout::Nhwc);
        assert_eq!(mask, [0, 255, 128]);
    }

    #[test]
    fn multiclass_logits_become_one_minus_background() {
        let mut values = [0.0f32; 12];
        values[0] = 10.0; // pixel 0: confident background
        values[6 + 3] = 10.0; // pixel 1: confident "face skin"
        let mut mask = [0u8; 2];
        read_output(&values, &mut mask, 2, OutputKind::Logits, Layout::Nhwc);
        assert_eq!(mask, [0, 255]);
    }

    #[test]
    fn alpha_output_is_used_directly_in_either_layout() {
        let mut mask = [0u8; 2];
        read_output(&[0.25, 1.5], &mut mask, 2, OutputKind::Alpha, Layout::Nchw);
        assert_eq!(mask, [64, 255]);
    }
}
