//! Building blocks: render targets, full-screen shader passes and readback.

use wgpu::util::DeviceExt;

use crate::{Gpu, GpuError};

const COMMON: &str = include_str!("shaders/common.wgsl");

/// Sixteen floats: the parameter block every pass receives.
pub type Params = [f32; 16];

/// How a pass reads one of its input textures.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Sampled with the linear sampler (8-bit and 16-bit float formats).
    Filtered,
    /// Read texel by texel (32-bit float formats, which cannot be filtered).
    Exact,
}

/// A texture that passes render into and later passes read from.
pub struct Target {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl Target {
    pub fn new(gpu: &Gpu, label: &str, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view, width, height }
    }

    /// One texel of this target, in uv units.
    pub fn texel(&self) -> [f32; 2] {
        [1.0 / self.width as f32, 1.0 / self.height as f32]
    }
}

/// One full-screen fragment shader with its own parameter buffer.
///
/// A pass that runs twice per frame with different parameters needs two
/// `Pass` values: parameter writes are queued, so one buffer can only hold
/// one value per submission.
pub struct Pass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    label: &'static str,
}

impl Pass {
    pub fn new(gpu: &Gpu, label: &'static str, fragment: &str, inputs: &[Input], output: wgpu::TextureFormat) -> Self {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{fragment}").into()),
        });

        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        for (i, input) in inputs.iter().enumerate() {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 + i as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: *input == Input::Filtered },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        let layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(output.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&[0.0f32; 16]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Self { pipeline, layout, params, label }
    }

    /// Queue this pass: read `inputs`, write `target`.
    pub fn draw(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        sampler: &wgpu::Sampler,
        inputs: &[&wgpu::TextureView],
        target: &wgpu::TextureView,
        params: Params,
    ) {
        gpu.queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ];
        for (i, view) in inputs.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(self.label),
            layout: &self.layout,
            entries: &entries,
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Copies an RGBA8 target back to system memory.
pub struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
    data: Vec<u8>,
}

impl Readback {
    pub fn new(gpu: &Gpu, label: &str, width: u32, height: u32) -> Self {
        // Rows in a copy must be padded to a multiple of 256 bytes.
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = (width * 4).div_ceil(align) * align;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: padded_row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { buffer, width, height, padded_row, data: vec![0; (width * height * 4) as usize] }
    }

    /// Queue the copy from `target`; call [`Self::fetch`] after submitting.
    pub fn copy_from(&self, encoder: &mut wgpu::CommandEncoder, target: &Target) {
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Wait for the graphics card and return the tightly packed bytes.
    pub fn fetch(&mut self, gpu: &Gpu) -> Result<&[u8], GpuError> {
        let slice = self.buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Readback(e.to_string()))?;
        receiver
            .recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        {
            let mapped = slice.get_mapped_range().map_err(|e| GpuError::Readback(e.to_string()))?;
            let row = (self.width * 4) as usize;
            if self.padded_row as usize == row {
                self.data.copy_from_slice(&mapped);
            } else {
                for (dst, src) in self.data.chunks_exact_mut(row).zip(mapped.chunks_exact(self.padded_row as usize)) {
                    dst.copy_from_slice(&src[..row]);
                }
            }
        }
        self.buffer.unmap();
        Ok(&self.data)
    }
}
