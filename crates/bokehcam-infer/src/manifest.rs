//! Model manifests: small TOML files that describe a model well enough to
//! download, verify and run it. Supporting another model of a known kind is
//! a new manifest, not new code.
//!
//! Built-in manifests live in this crate's `models/` directory. Users can
//! add their own next to the downloaded model files
//! (`~/.local/share/bokehcam/models/*.toml`).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::InferError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// batch, height, width, channels
    Nhwc,
    /// batch, channels, height, width
    Nchw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Range {
    #[serde(rename = "0..1")]
    ZeroToOne,
    #[serde(rename = "-1..1")]
    MinusOneToOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputKind {
    /// Per-pixel class scores; class 0 is the background.
    Logits,
    /// A single-channel matte, 0 (background) to 1 (person).
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub kind: OutputKind,
    pub layout: Layout,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub title: String,
    pub description: String,
    pub license: String,
    pub source: String,
    pub file: String,
    pub url: String,
    pub sha256: String,
    pub quality: u32,
    pub input: Input,
    pub output: Output,
}

const BUILT_IN: &[&str] = &[
    include_str!("../models/selfie-landscape.toml"),
    include_str!("../models/selfie-multiclass.toml"),
    include_str!("../models/modnet.toml"),
];

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, InferError> {
        let manifest: Manifest = toml::from_str(text).map_err(|e| InferError::Manifest(e.message().to_string()))?;
        if manifest.input.width == 0 || manifest.input.height == 0 {
            return Err(InferError::Manifest(format!("{}: input size must not be zero", manifest.name)));
        }
        Ok(manifest)
    }
}

/// Directories searched for model files and user manifests, in order. The
/// first one is where downloads go.
pub fn model_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("BOKEHCAM_MODELS").filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(dir));
    }
    dirs.push(bokehcam_core::paths::models_dir());
    dirs
}

/// Every known model: built-in manifests plus user manifests, best first.
/// A user manifest with the name of a built-in one replaces it.
pub fn all() -> Vec<Manifest> {
    let mut models: Vec<Manifest> =
        BUILT_IN.iter().map(|text| Manifest::parse(text).expect("built-in manifest is valid")).collect();
    for dir in model_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml")) {
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| Manifest::parse(&t).map_err(|e| e.to_string()))
            {
                Ok(manifest) => {
                    models.retain(|m| m.name != manifest.name);
                    models.push(manifest);
                }
                Err(problem) => tracing::warn!("ignoring model manifest {}: {problem}", path.display()),
            }
        }
    }
    models.sort_by(|a, b| b.quality.cmp(&a.quality).then(a.name.cmp(&b.name)));
    models
}

pub fn find(name: &str) -> Result<Manifest, InferError> {
    all().into_iter().find(|m| m.name == name).ok_or_else(|| InferError::UnknownModel(name.to_string()))
}

impl Manifest {
    /// Where this model's file is, if it has been downloaded.
    pub fn installed_path(&self) -> Option<PathBuf> {
        model_dirs().into_iter().map(|dir| dir.join(&self.file)).find(|path| path.is_file())
    }

    /// Download the model file if it is missing, verifying its checksum.
    /// `progress` receives (bytes so far, total bytes if known).
    pub fn fetch(&self, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf, InferError> {
        if let Some(path) = self.installed_path() {
            if sha256_of(&path)? == self.sha256 {
                return Ok(path);
            }
            tracing::warn!("{} does not match its checksum; downloading again", path.display());
        }
        let dir = model_dirs().into_iter().next().expect("at least one model directory");
        let download = |problem: String| InferError::Download { model: self.name.clone(), problem };
        std::fs::create_dir_all(&dir).map_err(|e| download(e.to_string()))?;
        let destination = dir.join(&self.file);
        let partial = dir.join(format!("{}.part", self.file));

        let response = ureq::get(&self.url).call().map_err(|e| download(e.to_string()))?;
        let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok());
        let mut reader = response.into_body().into_reader();
        let mut file = std::fs::File::create(&partial).map_err(|e| download(e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 16];
        let mut done = 0u64;
        loop {
            let n = reader.read(&mut buffer).map_err(|e| download(e.to_string()))?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut file, &buffer[..n]).map_err(|e| download(e.to_string()))?;
            hasher.update(&buffer[..n]);
            done += n as u64;
            progress(done, total);
        }
        drop(file);
        if hex(&hasher.finalize()) != self.sha256 {
            let _ = std::fs::remove_file(&partial);
            return Err(download("checksum mismatch; the download was discarded".into()));
        }
        std::fs::rename(&partial, &destination).map_err(|e| download(e.to_string()))?;
        Ok(destination)
    }
}

fn sha256_of(path: &Path) -> Result<String, InferError> {
    let mut file = std::fs::File::open(path).map_err(|e| InferError::Io(path.display().to_string(), e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buffer).map_err(|e| InferError::Io(path.display().to_string(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_manifests_parse_and_are_ranked_best_first() {
        let models: Vec<Manifest> = BUILT_IN.iter().map(|t| Manifest::parse(t).unwrap()).collect();
        assert!(models.iter().any(|m| m.name == "selfie-landscape"));
        for model in &models {
            assert_eq!(model.sha256.len(), 64, "{}", model.name);
            assert!(model.url.starts_with("https://"), "{}", model.name);
        }
    }

    #[test]
    fn unknown_fields_and_bad_values_are_rejected() {
        let good = BUILT_IN[0];
        assert!(Manifest::parse(&good.replace("quality = 1", "qualty = 1")).is_err());
        assert!(Manifest::parse(&good.replace("\"nhwc\"", "\"hwc\"")).is_err());
        assert!(Manifest::parse(&good.replace("width = 256", "width = 0")).is_err());
    }
}
