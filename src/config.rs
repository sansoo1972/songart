use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

/// Top-level application configuration loaded from TOML.
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub logging: LoggingConfig,
    pub audio: AudioConfig,
    pub paths: PathsConfig,
    pub display: DisplayConfig,
    pub display_presets: HashMap<String, DisplayPreset>,
    pub fonts: FontsConfig,
    pub font_themes: HashMap<String, FontTheme>,
    pub visualizer: VisualizerConfig,
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
}

/// High-level display settings.
#[derive(Debug, Deserialize, Clone)]
pub struct DisplayConfig {
    pub window_title: String,
    pub fullscreen: bool,
    pub orientation: String,
    pub frame_delay_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct VisualizerConfig {
    pub enabled: bool,
    pub mode: String,
    pub position: String,
    pub style: String,
    pub height: u32,
    pub padding: u32,
    pub peak_hold: bool,
    pub smoothing: f32,
}

/// A full named layout preset selected by `display.orientation`.
#[derive(Debug, Deserialize, Clone)]
pub struct DisplayPreset {
    pub width: u32,
    pub height: u32,
    pub top_panel_ratio: f32,
    pub panel_x: i32,
    pub panel_y: i32,
    pub title_line_spacing: i32,
    pub body_line_spacing: i32,
    pub detail_line_spacing: i32,
}

/// High-level font selection.
#[derive(Debug, Deserialize, Clone)]
pub struct FontsConfig {
    pub theme: String,
}

/// A single named font theme.
#[derive(Debug, Deserialize, Clone)]
pub struct FontTheme {
    pub title: String,
    pub body: String,
    pub title_size: u16,
    pub body_size: u16,
}

/// Loads application configuration from a TOML file.
pub fn load_config(path: &str) -> Result<AppConfig, String> {
    let raw =
        fs::read_to_string(path).map_err(|e| format!("Failed to read config {}: {e}", path))?;

    toml::from_str(&raw).map_err(|e| format!("Failed to parse config {}: {e}", path))
}