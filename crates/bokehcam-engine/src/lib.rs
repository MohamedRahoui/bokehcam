//! The running pipeline: camera → picture pipeline → virtual camera.
//!
//! [`Engine`] owns the devices and runs on its own threads. Front ends (the
//! command line, the settings window) only hold a handle: they read
//! [`Status`], send a new [`Config`], and may ask for preview pictures.

pub mod control;
mod devices;
mod engine;
#[cfg(test)]
mod fakes;
mod picture;
mod status;

pub use engine::Engine;
pub use status::{Preview, Stage, State, Status};

use bokehcam_core::ConfigError;
use bokehcam_gpu::GpuError;
use bokehcam_infer::InferError;
use bokehcam_media::MediaError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Gpu(#[from] GpuError),
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(transparent)]
    Infer(#[from] InferError),
}
