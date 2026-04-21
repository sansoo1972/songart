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

/// Audio capture and recognition settings.
///
/// This app now uses one continuous live audio capture stream and a shared
/// rolling in-memory buffer. Recognition snapshots are written from that
/// rolling buffer rather than recorded independently.
#[derive(Debug, Deserialize, Clone)]
pub struct AudioConfig {
    pub device: String,
    pub sample_wav: String,
    pub loop_delay_secs: u64,

    #[serde(default = "default_sample_rate")]
    pub sample_rate: usize,

    #[serde(default = "default_channels")]
    pub channels: usize,

    #[serde(default = "default_buffer_seconds")]
    pub buffer_seconds: usize,

    #[serde(default = "default_recognition_window_ms")]
    pub recognition_window_ms: usize,

    #[serde(default = "default_read_chunk_bytes")]
    pub read_chunk_bytes: usize,
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

/// Visualizer configuration.
///
/// These settings tune the live oscilloscope without requiring code changes.
#[derive(Debug, Deserialize, Clone)]
pub struct VisualizerConfig {
    pub enabled: bool,
    pub mode: String,

    #[serde(default)]
    pub position: String,

    #[serde(default)]
    pub style: String,

    pub height: u32,
    pub padding: u32,
    pub peak_hold: bool,
    pub smoothing: f32,

    #[serde(default = "default_window_ms")]
    pub window_ms: usize,

    #[serde(default = "default_point_count")]
    pub point_count: usize,

    #[serde(default = "default_gain")]
    pub gain: f32,

    #[serde(default = "default_y_scale")]
    pub y_scale: f32,

    #[serde(default = "default_left_y_offset")]
    pub left_y_offset: f32,

    #[serde(default = "default_right_y_offset")]
    pub right_y_offset: f32,

    #[serde(default = "default_visible_sample_count")]
    pub visible_sample_count: usize,

    #[serde(default = "default_max_gain")]
    pub max_gain: f32,

    #[serde(default = "default_debug_log_interval_ms")]
    pub debug_log_interval_ms: u64,
}

fn default_sample_rate() -> usize {
    16_000
}

fn default_channels() -> usize {
    1
}

fn default_buffer_seconds() -> usize {
    20
}

fn default_recognition_window_ms() -> usize {
    10_000
}

fn default_read_chunk_bytes() -> usize {
    4096
}

fn default_window_ms() -> usize {
    120
}

fn default_point_count() -> usize {
    160
}

fn default_gain() -> f32 {
    6.0
}

fn default_y_scale() -> f32 {
    0.75
}

fn default_left_y_offset() -> f32 {
    0.25
}

fn default_right_y_offset() -> f32 {
    0.75
}

fn default_visible_sample_count() -> usize {
    480
}

fn default_max_gain() -> f32 {
    24.0
}

fn default_debug_log_interval_ms() -> u64 {
    1000
}

/// Loads application configuration from a TOML file.
pub fn load_config(path: &str) -> Result<AppConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("Failed to read config {}: {e}", path))?;

    toml::from_str(&raw).map_err(|e| format!("Failed to parse config {}: {e}", path))
}