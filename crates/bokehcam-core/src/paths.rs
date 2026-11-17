//! Where bokehcam keeps its files (XDG base directories on Linux).

use std::env;
use std::path::PathBuf;

const APP: &str = "bokehcam";

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(fallback),
    }
}

/// `~/.config/bokehcam/config.toml`
pub fn config_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join(APP).join("config.toml")
}

/// `~/.local/share/bokehcam/models`: downloaded model files and user manifests.
pub fn models_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join(APP).join("models")
}

/// The control socket of the running service.
pub fn control_socket() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("bokehcam.sock"),
        _ => env::temp_dir().join(format!("bokehcam-{}.sock", env::var("USER").unwrap_or_default())),
    }
}
