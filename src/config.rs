use serde::Deserialize;
use std::fs;

/// Top-level application configuration loaded from TOML.
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub logging: LoggingConfig,
    pub audio: AudioConfig,
    pub paths: PathsConfig,
    pub display: DisplayConfig,
}

/// Logging configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
    pub reset_on_start: bool,
}

/// Audio capture and recognition timing settings.
#[derive(Debug, Deserialize, Clone)]
pub struct AudioConfig {
    pub device: String,
    pub sample_wav: String,
    pub record_seconds: u64,
    pub loop_delay_secs: u64,
}

/// Filesystem paths used by the application.
#[derive(Debug, Deserialize, Clone)]
pub struct PathsConfig {
    pub songrec_bin: String,
    pub artwork_file: String,
    pub font_path: String,
}

/// Display and rendering settings.
#[derive(Debug, Deserialize, Clone)]
pub struct DisplayConfig {
    pub window_title: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub top_panel_ratio: f32,
    pub title_font_size: u16,
    pub body_font_size: u16,
    pub frame_delay_ms: u64,
}

/// Loads application configuration from a TOML file.
pub fn load_config(path: &str) -> Result<AppConfig, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config {}: {e}", path))?;

    toml::from_str(&raw)
        .map_err(|e| format!("Failed to parse config {}: {e}", path))
}