//! The frame pipeline: an uploaded camera frame in, a YUYV frame out.
//!
//! ```text
//! source ─frame─► framed ─┬──────────────────────────────────────────► composite ─► final ─yuyv─► packed
//!                         └─down4─► small ─┬─copy─► model input ─(model)─► raw mask        ▲
//!                                          │                       temporal ─► mask         │
//!                                          ├─ guided filter (moments, box, coeff, box) ─► coeff
//!                                          └─ bg_prepare ─► gauss ─► gauss ─► blurred ──────┘
//! ```

use wgpu::TextureFormat as Format;

use crate::pass::{Input, Params, Pass, Readback, Target};
use crate::{Gpu, GpuError};

/// The small picture (mask statistics, blur source) is this much smaller than the output.
const SMALL_FACTOR: u32 = 4;
/// Mask changes below/above these are treated as pure noise / pure motion.
const NOISE_CHANGE: f32 = 0.04;
const MOTION_CHANGE: f32 = 0.30;

/// The resolutions a [`Renderer`] is built for. Changing any of them means
/// building a new renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    /// Camera frame.
    pub source: (u32, u32),
    /// Output picture (both even).
    pub output: (u32, u32),
    /// Model input; `None` when no person detection is used.
    pub model: Option<(u32, u32)>,
}

impl Sizes {
    pub fn small(&self) -> (u32, u32) {
        ((self.output.0 / SMALL_FACTOR).max(8), (self.output.1 / SMALL_FACTOR).max(8))
    }
}

/// Everything that can change from one frame to the next without rebuilding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameParams {
    pub zoom: f32,
    pub center: (f32, f32),
    pub mirror: bool,
    /// Blur the background (requires a model size and a mask each frame).
    pub blur: bool,
    /// Blur radius, percent of the picture height.
    pub blur_percent: f32,
    pub temporal_smoothing: f32,
    /// Edge snapping range, percent of the picture height.
    pub edge_radius_percent: f32,
    pub edge_eps: f32,
    pub cutoff_low: f32,
    pub cutoff_high: f32,
    /// Grow (+) or shrink (-) the outline, in output pixels.
    pub grow: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
}

/// The crop rectangle `(x, y, width, height)` in source uv: the largest area
/// with the output's aspect ratio, divided by `zoom`, centred on `center`
/// and clamped to stay inside the frame.
pub fn crop_rect(source: (u32, u32), output: (u32, u32), zoom: f32, center: (f32, f32)) -> [f32; 4] {
    let (sw, sh) = (source.0 as f32, source.1 as f32);
    let aspect = output.0 as f32 / output.1 as f32;
    let base_w = sw.min(sh * aspect);
    let base_h = base_w / aspect;
    let w = base_w / zoom.max(1.0) / sw;
    let h = base_h / zoom.max(1.0) / sh;
    let x = (center.0 - w / 2.0).clamp(0.0, 1.0 - w);
    let y = (center.1 - h / 2.0).clamp(0.0, 1.0 - h);
    [x, y, w, h]
}

struct MaskStages {
    model_size: (u32, u32),
    model_input: Target,
    model_readback: Readback,
    raw_mask: Target,
    /// Smoothed masks, alternating between "current" and "previous".
    masks: [Target; 2],
    current: usize,
    has_history: bool,
    small: Target,
    moments: Target,
    scratch32: Target,
    means: Target,
    coeff32: Target,
    coeff: Target,
    half_a: Target,
    half_b: Target,
    down4: Pass,
    to_model: Pass,
    temporal: Pass,
    gf_moments: Pass,
    box_h1: Pass,
    box_v1: Pass,
    gf_coeff: Pass,
    box_h2: Pass,
    box_v2: Pass,
    bg_prepare: Pass,
    gauss_h: Pass,
    gauss_v: Pass,
}

pub struct Renderer {
    sizes: Sizes,
    sampler: wgpu::Sampler,
    source: Target,
    framed: Target,
    finished: Target,
    packed: Target,
    packed_readback: Readback,
    /// A 1x1 texture bound where a pass input is unused.
    blank: Target,
    frame: Pass,
    composite: Pass,
    yuyv: Pass,
    preview_pass: Pass,
    preview: Option<(Target, Readback)>,
    mask: Option<MaskStages>,
}

impl Renderer {
    pub fn new(gpu: &Gpu, sizes: Sizes) -> Self {
        let rgba8 = Format::Rgba8Unorm;
        let (ow, oh) = sizes.output;
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let one = |source| Pass::new(gpu, "pass", source, &[Input::Filtered], rgba8);
        Self {
            sizes,
            sampler,
            source: Target::new(gpu, "source", sizes.source.0, sizes.source.1, rgba8),
            framed: Target::new(gpu, "framed", ow, oh, rgba8),
            finished: Target::new(gpu, "final", ow, oh, rgba8),
            packed: Target::new(gpu, "packed", ow / 2, oh, rgba8),
            packed_readback: Readback::new(gpu, "packed", ow / 2, oh),
            blank: Target::new(gpu, "blank", 1, 1, Format::Rgba16Float),
            frame: one(include_str!("shaders/frame.wgsl")),
            composite: Pass::new(
                gpu,
                "composite",
                include_str!("shaders/composite.wgsl"),
                &[Input::Filtered; 3],
                rgba8,
            ),
            yuyv: one(include_str!("shaders/yuyv.wgsl")),
            preview_pass: one(include_str!("shaders/copy.wgsl")),
            preview: None,
            mask: sizes.model.map(|model| MaskStages::new(gpu, &sizes, model)),
        }
    }

    pub fn sizes(&self) -> Sizes {
        self.sizes
    }

    /// Forget the previous mask (call when the camera restarts).
    pub fn reset(&mut self) {
        if let Some(mask) = &mut self.mask {
            mask.has_history = false;
        }
    }

    /// Upload a decoded camera frame (tightly packed RGBA, source size).
    pub fn upload(&self, gpu: &Gpu, rgba: &[u8]) {
        let (width, height) = self.sizes.source;
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        gpu.queue.write_texture(
            self.source.texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }

    /// Frame the uploaded picture. When the background is blurred, also
    /// return the model's input image (RGBA, model size).
    pub fn prepare(&mut self, gpu: &Gpu, params: &FrameParams) -> Result<Option<&[u8]>, GpuError> {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let crop = crop_rect(self.sizes.source, self.sizes.output, params.zoom, params.center);
        let shrinking = crop[2] * self.sizes.source.0 as f32 > self.sizes.output.0 as f32 * 1.01;
        let quarter = [crop[2] / self.sizes.output.0 as f32 * 0.25, crop[3] / self.sizes.output.1 as f32 * 0.25];
        self.frame.draw(
            gpu,
            &mut encoder,
            &self.sampler,
            &[&self.source.view],
            &self.framed.view,
            params16(&[
                crop[0],
                crop[1],
                crop[2],
                crop[3],
                params.mirror as u8 as f32,
                shrinking as u8 as f32,
                quarter[0],
                quarter[1],
            ]),
        );
        let mask = match &mut self.mask {
            Some(mask) if params.blur => mask,
            _ => {
                gpu.queue.submit([encoder.finish()]);
                return Ok(None);
            }
        };
        let texel = self.framed.texel();
        mask.down4.draw(gpu, &mut encoder, &self.sampler, &[&self.framed.view], &mask.small.view, params16(&texel));
        mask.to_model.draw(
            gpu,
            &mut encoder,
            &self.sampler,
            &[&mask.small.view],
            &mask.model_input.view,
            params16(&[]),
        );
        mask.model_readback.copy_from(&mut encoder, &mask.model_input);
        gpu.queue.submit([encoder.finish()]);
        mask.model_readback.fetch(gpu).map(Some)
    }

    /// Provide the model's answer for the frame given to [`Self::prepare`]:
    /// person probability per model pixel, 0..255, row by row.
    pub fn set_mask(&self, gpu: &Gpu, probabilities: &[u8]) {
        let Some(mask) = &self.mask else { return };
        let (width, height) = mask.model_size;
        debug_assert_eq!(probabilities.len(), (width * height) as usize);
        gpu.queue.write_texture(
            mask.raw_mask.texture.as_image_copy(),
            probabilities,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }

    /// Composite, colour and pack. Returns the finished frame as YUYV.
    pub fn finish(&mut self, gpu: &Gpu, params: &FrameParams) -> Result<&[u8], GpuError> {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let grade = [params.brightness, params.contrast, params.saturation, 1.0 / params.gamma.max(0.01)];
        match &mut self.mask {
            Some(mask) if params.blur => {
                mask.refine(gpu, &mut encoder, &self.sampler, params, self.sizes.output.1);
                self.composite.draw(
                    gpu,
                    &mut encoder,
                    &self.sampler,
                    &[&self.framed.view, &mask.coeff.view, &mask.half_a.view],
                    &self.finished.view,
                    params16(&[grade[0], grade[1], grade[2], grade[3], 1.0]),
                );
            }
            _ => self.composite.draw(
                gpu,
                &mut encoder,
                &self.sampler,
                &[&self.framed.view, &self.blank.view, &self.blank.view],
                &self.finished.view,
                params16(&[grade[0], grade[1], grade[2], grade[3], 0.0]),
            ),
        }
        self.yuyv.draw(gpu, &mut encoder, &self.sampler, &[&self.finished.view], &self.packed.view, params16(&[]));
        self.packed_readback.copy_from(&mut encoder, &self.packed);
        gpu.queue.submit([encoder.finish()]);
        self.packed_readback.fetch(gpu)
    }

    /// The most recently finished picture, scaled to `width` x `height`, as RGBA.
    pub fn preview(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<&[u8], GpuError> {
        if !matches!(&self.preview, Some((target, _)) if (target.width, target.height) == (width, height)) {
            let target = Target::new(gpu, "preview", width, height, Format::Rgba8Unorm);
            self.preview = Some((target, Readback::new(gpu, "preview", width, height)));
        }
        let (target, readback) = self.preview.as_mut().expect("just created");
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.preview_pass.draw(gpu, &mut encoder, &self.sampler, &[&self.finished.view], &target.view, params16(&[]));
        readback.copy_from(&mut encoder, target);
        gpu.queue.submit([encoder.finish()]);
        readback.fetch(gpu)
    }
}

impl MaskStages {
    fn new(gpu: &Gpu, sizes: &Sizes, model: (u32, u32)) -> Self {
        let (sw, sh) = sizes.small();
        let (hw, hh) = ((sw / 2).max(4), (sh / 2).max(4));
        let f32x4 = Format::Rgba32Float;
        let f16x4 = Format::Rgba16Float;
        let rgba8 = Format::Rgba8Unorm;
        let filtered = |label, source, inputs: usize, output| {
            Pass::new(gpu, label, source, &vec![Input::Filtered; inputs], output)
        };
        let exact = |label, source, output| Pass::new(gpu, label, source, &[Input::Exact], output);
        let box_blur = include_str!("shaders/box.wgsl");
        let gauss = include_str!("shaders/gauss.wgsl");
        Self {
            model_size: model,
            model_input: Target::new(gpu, "model input", model.0, model.1, rgba8),
            model_readback: Readback::new(gpu, "model input", model.0, model.1),
            raw_mask: Target::new(gpu, "raw mask", model.0, model.1, Format::R8Unorm),
            masks: [
                Target::new(gpu, "mask a", model.0, model.1, Format::R16Float),
                Target::new(gpu, "mask b", model.0, model.1, Format::R16Float),
            ],
            current: 0,
            has_history: false,
            small: Target::new(gpu, "small", sw, sh, rgba8),
            moments: Target::new(gpu, "moments", sw, sh, f32x4),
            scratch32: Target::new(gpu, "scratch", sw, sh, f32x4),
            means: Target::new(gpu, "means", sw, sh, f32x4),
            coeff32: Target::new(gpu, "coeff32", sw, sh, f32x4),
            coeff: Target::new(gpu, "coeff", sw, sh, f16x4),
            half_a: Target::new(gpu, "blur a", hw, hh, f16x4),
            half_b: Target::new(gpu, "blur b", hw, hh, f16x4),
            down4: filtered("down4", include_str!("shaders/down4.wgsl"), 1, rgba8),
            to_model: filtered("to model", include_str!("shaders/copy.wgsl"), 1, rgba8),
            temporal: filtered("temporal", include_str!("shaders/temporal.wgsl"), 2, Format::R16Float),
            gf_moments: filtered("gf moments", include_str!("shaders/gf_moments.wgsl"), 2, f32x4),
            box_h1: exact("box h1", box_blur, f32x4),
            box_v1: exact("box v1", box_blur, f32x4),
            gf_coeff: exact("gf coeff", include_str!("shaders/gf_coeff.wgsl"), f32x4),
            box_h2: exact("box h2", box_blur, f32x4),
            box_v2: exact("box v2", box_blur, f16x4),
            bg_prepare: filtered("bg prepare", include_str!("shaders/bg_prepare.wgsl"), 2, f16x4),
            gauss_h: filtered("gauss h", gauss, 1, f16x4),
            gauss_v: filtered("gauss v", gauss, 1, f16x4),
        }
    }

    /// Mask smoothing, edge refinement and the blurred background. Leaves
    /// the results in `coeff` and `half_a`.
    fn refine(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        sampler: &wgpu::Sampler,
        params: &FrameParams,
        output_height: u32,
    ) {
        let small_h = self.small.height as f32;
        let previous = self.current;
        self.current = 1 - self.current;
        let texel = self.raw_mask.texel();
        self.temporal.draw(
            gpu,
            encoder,
            sampler,
            &[&self.raw_mask.view, &self.masks[previous].view],
            &self.masks[self.current].view,
            params16(&[
                params.temporal_smoothing,
                NOISE_CHANGE,
                MOTION_CHANGE,
                self.has_history as u8 as f32,
                texel[0],
                texel[1],
            ]),
        );
        self.has_history = true;
        let mask = &self.masks[self.current].view;

        // Guided filter at the small size.
        let small_texel = self.small.texel();
        let grow = params.grow * small_h / output_height as f32;
        self.gf_moments.draw(
            gpu,
            encoder,
            sampler,
            &[&self.small.view, mask],
            &self.moments.view,
            params16(&[grow, 0.0, small_texel[0], small_texel[1]]),
        );
        let radius = (params.edge_radius_percent / 100.0 * small_h).round().max(1.0);
        let horizontal = params16(&[1.0, 0.0, radius]);
        let vertical = params16(&[0.0, 1.0, radius]);
        self.box_h1.draw(gpu, encoder, sampler, &[&self.moments.view], &self.scratch32.view, horizontal);
        self.box_v1.draw(gpu, encoder, sampler, &[&self.scratch32.view], &self.means.view, vertical);
        self.gf_coeff.draw(
            gpu,
            encoder,
            sampler,
            &[&self.means.view],
            &self.coeff32.view,
            params16(&[params.edge_eps, params.cutoff_low, params.cutoff_high]),
        );
        self.box_h2.draw(gpu, encoder, sampler, &[&self.coeff32.view], &self.scratch32.view, horizontal);
        self.box_v2.draw(gpu, encoder, sampler, &[&self.scratch32.view], &self.coeff.view, vertical);

        // Blurred background at half the small size.
        self.bg_prepare.draw(
            gpu,
            encoder,
            sampler,
            &[&self.small.view, &self.coeff.view],
            &self.half_a.view,
            params16(&small_texel),
        );
        let sigma = (params.blur_percent / 100.0 * self.half_a.height as f32).max(0.5);
        let half_texel = self.half_a.texel();
        self.gauss_h.draw(
            gpu,
            encoder,
            sampler,
            &[&self.half_a.view],
            &self.half_b.view,
            params16(&[half_texel[0], 0.0, sigma]),
        );
        self.gauss_v.draw(
            gpu,
            encoder,
            sampler,
            &[&self.half_b.view],
            &self.half_a.view,
            params16(&[0.0, half_texel[1], sigma]),
        );
    }
}

fn params16(values: &[f32]) -> Params {
    let mut params = [0.0; 16];
    params[..values.len()].copy_from_slice(values);
    params
}

#[cfg(test)]
mod tests {
    use super::crop_rect;

    fn close(a: [f32; 4], b: [f32; 4]) {
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-5, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn full_view_uses_the_whole_frame() {
        close(crop_rect((3840, 2160), (1920, 1080), 1.0, (0.5, 0.5)), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn zoom_crops_around_the_centre() {
        close(crop_rect((3840, 2160), (1920, 1080), 1.25, (0.5, 0.5)), [0.1, 0.1, 0.8, 0.8]);
    }

    #[test]
    fn the_crop_is_clamped_inside_the_frame() {
        close(crop_rect((1920, 1080), (1920, 1080), 2.0, (1.0, 0.0)), [0.5, 0.0, 0.5, 0.5]);
    }

    #[test]
    fn a_different_aspect_ratio_crops_instead_of_stretching() {
        // 4:3 source, 16:9 output: full width, 3/4 of the height, centred.
        close(crop_rect((1600, 1200), (1280, 720), 1.0, (0.5, 0.5)), [0.0, 0.125, 1.0, 0.75]);
    }
}
