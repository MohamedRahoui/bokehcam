//! Configuration, the settings schema, and types shared by every bokehcam crate.
//!
//! This crate has no platform or GPU dependencies, so tools and front ends
//! can depend on it cheaply.

pub mod config;
pub mod paths;
pub mod schema;

pub use config::{Config, ConfigError};
