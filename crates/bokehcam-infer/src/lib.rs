//! Models: what exists, where the files are, and running them on the
//! processor or the graphics card.

pub mod manifest;
mod segmenter;

pub use manifest::Manifest;
pub use segmenter::{Backend, Choice, Segmenter, gpu_available};

#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("unknown model `{0}` (see `bokehcam models`)")]
    UnknownModel(String),
    #[error("model `{0}` is not downloaded yet (run `bokehcam models fetch`)")]
    NotInstalled(String),
    #[error("no model is downloaded yet (run `bokehcam models fetch`)")]
    NothingInstalled,
    #[error("invalid model manifest: {0}")]
    Manifest(String),
    #[error("cannot download model `{model}`: {problem}")]
    Download { model: String, problem: String },
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
    #[error("model `{model}` on {backend}: {problem}")]
    Runtime { model: String, backend: &'static str, problem: String },
}
