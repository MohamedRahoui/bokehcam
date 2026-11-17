//! The configuration: typed, strictly validated, and addressable by key.
//!
//! Unknown keys and out-of-range values are errors, so a typo never silently
//! means "default". Ranges come from [`crate::schema`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use toml::Value;

use crate::schema::{self, Apply, Kind};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read { path: String, source: std::io::Error },
    #[error("cannot write {path}: {source}")]
    Write { path: String, source: std::io::Error },
    #[error("{0}")]
    Parse(String),
    #[error("{key}: {problem}")]
    Invalid { key: String, problem: String },
    #[error("unknown setting `{0}`")]
    UnknownKey(String),
}

fn invalid(key: &str, problem: impl Into<String>) -> ConfigError {
    ConfigError::Invalid { key: key.to_string(), problem: problem.into() }
}

macro_rules! section {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty = $default:expr),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }

        impl Default for $name {
            fn default() -> Self { Self { $($field: $default),* } }
        }
    };
}

section!(Camera {
    device: String = "auto".into(),
    width: u32 = 3840,
    height: u32 = 2160,
    fps: u32 = 30,
    format: String = "auto".into(),
    /// Hardware controls applied whenever the camera opens (names as listed
    /// by `bokehcam devices --controls`).
    controls: BTreeMap<String, i64> = BTreeMap::new(),
});

section!(Output {
    device: String = "auto".into(),
    width: u32 = 1920,
    height: u32 = 1080,
    on_demand: bool = true,
    idle_timeout: f64 = 3.0,
});

section!(Framing { zoom: f64 = 1.0, center_x: f64 = 0.5, center_y: f64 = 0.5, mirror: bool = false });

section!(Background { mode: String = "blur".into(), blur: f64 = 2.2 });

section!(Segmentation { model: String = "auto".into(), backend: String = "auto".into(), threads: u32 = 2 });

section!(Mask {
    temporal_smoothing: f64 = 0.6,
    edge_radius: f64 = 1.5,
    edge_eps: f64 = 0.002,
    cutoff_low: f64 = 0.25,
    cutoff_high: f64 = 0.75,
    grow: i32 = 0,
});

section!(Color { brightness: f64 = 0.0, contrast: f64 = 1.0, saturation: f64 = 1.0, gamma: f64 = 1.0 });

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub camera: Camera,
    pub output: Output,
    pub framing: Framing,
    pub background: Background,
    pub segmentation: Segmentation,
    pub mask: Mask,
    pub color: Color,
}

impl Config {
    /// Load and validate a config file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|source| ConfigError::Read { path: path.display().to_string(), source })?;
        Self::from_toml(&text)
    }

    /// Load `path` if it exists, otherwise the built-in defaults.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() { Self::load(path) } else { Ok(Self::default()) }
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|e| ConfigError::Parse(e.message().to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Check every value against the schema, plus the cross-field rules.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let tree = self.tree();
        for setting in schema::GROUPS.iter().flat_map(|g| g.settings) {
            let value = lookup(&tree, setting.key).expect("schema key missing from Config");
            check(setting.key, &setting.kind, value)?;
        }
        if self.mask.cutoff_low >= self.mask.cutoff_high {
            return Err(invalid("mask.cutoff_low", "must be smaller than mask.cutoff_high"));
        }
        for (key, side) in [("output.width", self.output.width), ("output.height", self.output.height)] {
            if side % 2 != 0 {
                return Err(invalid(key, "must be an even number"));
            }
        }
        Ok(())
    }

    fn tree(&self) -> Value {
        Value::try_from(self).expect("Config always serialises")
    }

    /// The current value of a setting, by dotted key.
    pub fn get(&self, key: &str) -> Result<Value, ConfigError> {
        lookup(&self.tree(), key).cloned().ok_or_else(|| ConfigError::UnknownKey(key.to_string()))
    }

    /// A copy of this config with one setting changed, validated as a whole.
    pub fn with(&self, key: &str, value: Value) -> Result<Self, ConfigError> {
        let mut tree = self.tree();
        let slot = lookup_mut(&mut tree, key).ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
        // Whole numbers are accepted where the field is a float.
        *slot = match (&*slot, value) {
            (Value::Float(_), Value::Integer(i)) => Value::Float(i as f64),
            (_, v) => v,
        };
        let config: Config = tree.try_into().map_err(|e: toml::de::Error| invalid(key, e.message()))?;
        config.validate()?;
        Ok(config)
    }

    /// The most disruptive restart needed to go from `self` to `new`.
    pub fn apply_level(&self, new: &Config) -> Option<Apply> {
        if self == new {
            return None;
        }
        let (old_tree, new_tree) = (self.tree(), new.tree());
        let mut level = Apply::Live;
        for setting in schema::GROUPS.iter().flat_map(|g| g.settings) {
            if lookup(&old_tree, setting.key) != lookup(&new_tree, setting.key) {
                level = level.max(setting.apply);
            }
        }
        if self.camera.controls != new.camera.controls {
            level = level.max(Apply::ReopenCamera);
        }
        Some(level)
    }

    /// The config as a TOML file with every setting documented from the schema.
    pub fn to_toml(&self) -> String {
        let tree = self.tree();
        let mut out = String::from(
            "# bokehcam configuration. A running bokehcam reloads this file when it is saved.\n\
             # The settings window writes the same file.\n",
        );
        for group in schema::GROUPS {
            let _ = write!(out, "\n# {}: {}\n[{}]\n", group.title, group.help, group.id);
            for setting in group.settings {
                let name = setting.key.rsplit('.').next().unwrap_or(setting.key);
                let value = lookup(&tree, setting.key).expect("schema key missing from Config");
                let _ = writeln!(out, "# {}", wrap(&describe(setting), 76).join("\n# "));
                let _ = writeln!(out, "{name} = {value}");
            }
            if group.id == "camera" {
                out.push_str(
                    "\n# Hardware controls applied whenever the camera opens, so the picture never\n\
                     # depends on what another app left behind. List them with\n\
                     # `bokehcam devices --controls`.\n[camera.controls]\n",
                );
                for (name, value) in &self.camera.controls {
                    let _ = writeln!(out, "{name} = {value}");
                }
            }
        }
        out
    }

    /// Write the documented config file, creating its directory.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let wrap_err = |source| ConfigError::Write { path: path.display().to_string(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(wrap_err)?;
        }
        // Write-then-rename, so a watcher never sees a half-written file.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml()).map_err(wrap_err)?;
        std::fs::rename(&tmp, path).map_err(wrap_err)
    }
}

fn describe(setting: &schema::Setting) -> String {
    let range = match setting.kind {
        Kind::Int { min, max, .. } => format!(" ({min} to {max})"),
        Kind::Float { min, max, .. } => format!(" ({min} to {max})"),
        Kind::Choice(options) => {
            format!(" (one of: {})", options.iter().map(|(v, _)| format!("\"{v}\"")).collect::<Vec<_>>().join(", "))
        }
        _ => String::new(),
    };
    format!("{}: {}{range}", setting.label, setting.help)
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

fn lookup<'a>(tree: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(tree, |node, part| node.get(part))
}

fn lookup_mut<'a>(tree: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    key.split('.').try_fold(tree, |node, part| node.get_mut(part))
}

fn check(key: &str, kind: &Kind, value: &Value) -> Result<(), ConfigError> {
    match (*kind, value) {
        (Kind::Bool, Value::Boolean(_)) => Ok(()),
        (Kind::Int { min, max, .. }, Value::Integer(v)) if (min..=max).contains(v) => Ok(()),
        (Kind::Int { min, max, .. }, _) => Err(invalid(key, format!("must be a whole number from {min} to {max}"))),
        (Kind::Float { min, max, .. }, Value::Float(v)) if (min..=max).contains(v) => Ok(()),
        (Kind::Float { min, max, .. }, _) => Err(invalid(key, format!("must be a number from {min} to {max}"))),
        (Kind::Choice(options), Value::String(v)) if options.iter().any(|(o, _)| o == v) => Ok(()),
        (Kind::Choice(options), _) => {
            let allowed = options.iter().map(|(v, _)| format!("\"{v}\"")).collect::<Vec<_>>().join(", ");
            Err(invalid(key, format!("must be one of {allowed}")))
        }
        (Kind::Camera | Kind::VirtualCamera | Kind::Model | Kind::Text, Value::String(v)) if !v.is_empty() => Ok(()),
        (Kind::Camera | Kind::VirtualCamera | Kind::Model | Kind::Text, _) => Err(invalid(key, "must not be empty")),
        (Kind::Bool, _) => Err(invalid(key, "must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_round_trip_through_the_documented_file() {
        let config = Config::default();
        config.validate().unwrap();
        let text = config.to_toml();
        assert_eq!(Config::from_toml(&text).unwrap(), config);
        assert!(text.contains("# Blur strength:"), "settings are documented in the file");
    }

    #[test]
    fn every_schema_key_exists_in_the_config_and_vice_versa() {
        let tree = Config::default().tree();
        let mut schema_keys: Vec<String> =
            schema::GROUPS.iter().flat_map(|g| g.settings).map(|s| s.key.to_string()).collect();
        let mut config_keys = Vec::new();
        for (group, table) in tree.as_table().unwrap() {
            for (name, value) in table.as_table().unwrap() {
                if !value.is_table() {
                    config_keys.push(format!("{group}.{name}"));
                }
            }
        }
        schema_keys.sort();
        config_keys.sort();
        assert_eq!(schema_keys, config_keys);
    }

    #[test]
    fn partial_files_keep_defaults_for_the_rest() {
        let config = Config::from_toml("[framing]\nzoom = 1.25\n[camera.controls]\nzoom_absolute = 150\n").unwrap();
        assert_eq!(config.framing.zoom, 1.25);
        assert_eq!(config.camera.controls["zoom_absolute"], 150);
        assert_eq!(config.output, Output::default());
    }

    #[test]
    fn mistakes_are_rejected() {
        for bad in [
            "[nonsense]\n",
            "[framing]\nzom = 1.2\n",
            "[framing]\nzoom = 0.5\n",
            "[framing]\nzoom = \"big\"\n",
            "[framing]\nmirror = 1\n",
            "[camera]\nformat = \"h264\"\n",
            "[background]\nmode = \"replace\"\n",
            "[mask]\ncutoff_low = 0.8\ncutoff_high = 0.5\n",
            "[output]\nwidth = 1281\n",
            "[framing\nzoom = 1",
        ] {
            assert!(Config::from_toml(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn get_and_with_address_settings_by_key() {
        let config = Config::default();
        assert_eq!(config.get("background.blur").unwrap(), Value::Float(2.2));
        let changed = config.with("background.blur", Value::Integer(4)).unwrap();
        assert_eq!(changed.background.blur, 4.0);
        assert!(config.with("background.blur", Value::Float(99.0)).is_err());
        assert!(config.with("background.nope", Value::Float(1.0)).is_err());
        assert!(config.with("mask.cutoff_low", Value::Float(0.9)).is_err(), "cross-field rule applies");
    }

    #[test]
    fn apply_level_reports_the_most_disruptive_change() {
        let base = Config::default();
        assert_eq!(base.apply_level(&base), None);
        let blur = base.with("background.blur", Value::Float(3.0)).unwrap();
        assert_eq!(base.apply_level(&blur), Some(Apply::Live));
        let both = blur.with("output.width", Value::Integer(1280)).unwrap();
        assert_eq!(base.apply_level(&both), Some(Apply::ReopenOutput));
        let model = base.with("segmentation.backend", Value::String("cpu".into())).unwrap();
        assert_eq!(base.apply_level(&model), Some(Apply::ReloadModel));
    }
}
