//! The GPU picture pipeline.
//!
//! Everything that touches pixels at picture resolution runs here as shader
//! passes: framing, mask smoothing and edge refinement, background blur,
//! compositing, colour and packing for the virtual camera. The CPU only
//! decodes the camera frame and runs (or hands off) the model.

mod pass;
mod renderer;
mod shaders;

pub use renderer::{FrameParams, Renderer, Sizes};

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no usable graphics adapter found (a Vulkan, Metal or DirectX 12 driver is required)")]
    NoAdapter,
    #[error("cannot initialise the graphics device: {0}")]
    Device(String),
    #[error("reading a result back from the graphics card failed: {0}")]
    Readback(String),
}

/// What the picture pipeline runs on.
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub name: String,
    pub backend: String,
    pub kind: String,
    pub driver: String,
}

/// A graphics device and its command queue.
pub struct Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    info: AdapterInfo,
}

impl Gpu {
    /// Open the most capable adapter (a discrete card is preferred over an
    /// integrated one, and either over a software renderer).
    pub fn new() -> Result<Self, GpuError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|_| GpuError::NoAdapter)?;
        let raw = adapter.get_info();
        let info = AdapterInfo {
            name: raw.name.clone(),
            backend: format!("{:?}", raw.backend),
            kind: format!("{:?}", raw.device_type),
            driver: format!("{} {}", raw.driver, raw.driver_info).trim().to_string(),
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("bokehcam"),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Device(e.to_string()))?;
        tracing::info!("graphics: {} ({}, {})", info.name, info.backend, info.kind);
        Ok(Self { device, queue, info })
    }

    pub fn info(&self) -> &AdapterInfo {
        &self.info
    }

    /// Every adapter on this machine, for diagnostics.
    pub fn list_adapters() -> Vec<AdapterInfo> {
        let instance = wgpu::Instance::default();
        pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
            .into_iter()
            .map(|adapter| {
                let raw = adapter.get_info();
                AdapterInfo {
                    name: raw.name,
                    backend: format!("{:?}", raw.backend),
                    kind: format!("{:?}", raw.device_type),
                    driver: format!("{} {}", raw.driver, raw.driver_info).trim().to_string(),
                }
            })
            .collect()
    }
}
