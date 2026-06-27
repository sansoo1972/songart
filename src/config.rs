use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

// ==============================================================================
// Top-Level Config
// ==============================================================================

/// Top-level application configuration loaded from TOML.
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub logging: LoggingConfig,
    pub audio: AudioConfig,
    pub paths: PathsConfig,
    pub display: DisplayConfig,
    #[serde(default)]
    pub artwork: ArtworkConfig,
    pub display_presets: HashMap<String, DisplayPreset>,
    pub fonts: FontsConfig,
    pub font_themes: HashMap<String, FontTheme>,
    pub visualizer: VisualizerConfig,
}

// ==============================================================================
// Artwork
// ==============================================================================

/// Controls how album artwork is presented.
#[derive(Debug, Deserialize, Clone)]
pub struct ArtworkConfig {
    /// `cover` keeps the original rectangular presentation; `turntable` renders
    /// the artwork as the center label of a vinyl record.
    #[serde(default = "default_artwork_mode")]
    pub mode: String,
}

impl Default for ArtworkConfig {
    fn default() -> Self {
        Self {
            mode: default_artwork_mode(),
        }
    }
}

// ==============================================================================
// Logging
// ==============================================================================

/// Logging configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
    pub reset_on_start: bool,
}

// ==============================================================================
// Audio + Recognition
// ==============================================================================

/// Audio capture and recognition settings.
#[derive(Debug, Deserialize, Clone)]
pub struct AudioConfig {
    /// PulseAudio / PipeWire source or monitor name.
    pub device: String,

    /// Path used for SongRec recognition snapshots.
    pub sample_wav: String,

    /// Delay between recognition attempts.
    pub loop_delay_secs: u64,

    /// Continuous capture sample rate.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: usize,

    /// Number of audio channels to capture.
    #[serde(default = "default_channels")]
    pub channels: usize,

    /// Rolling capture buffer size in seconds.
    #[serde(default = "default_buffer_seconds")]
    pub buffer_seconds: usize,

    /// Amount of recent buffered audio written to WAV for SongRec.
    #[serde(default = "default_recognition_window_ms")]
    pub recognition_window_ms: usize,

    /// Size of read chunks from the live capture stream.
    #[serde(default = "default_read_chunk_bytes")]
    pub read_chunk_bytes: usize,
}

// ==============================================================================
// Paths
// ==============================================================================

/// Filesystem paths used by the application.
#[derive(Debug, Deserialize, Clone)]
pub struct PathsConfig {
    /// Local SongRec binary used for recognition.
    pub songrec_bin: String,

    /// Current artwork image written by recognition and read by the renderer.
    pub artwork_file: String,
}

// ==============================================================================
// Display
// ==============================================================================

/// High-level display settings.
#[derive(Debug, Deserialize, Clone)]
pub struct DisplayConfig {
    pub window_title: String,
    pub fullscreen: bool,
    pub orientation: String,
    pub frame_delay_ms: u64,

    /// Configurable colors for the major display regions.
    #[serde(default)]
    pub colors: DisplayColorsConfig,
}

/// Configurable display region colors.
///
/// Values should be hex strings such as `#000000`, `#080808`, or `#101014`.
#[derive(Debug, Deserialize, Clone)]
pub struct DisplayColorsConfig {
    /// Overall canvas background.
    #[serde(default = "default_black_color")]
    pub background: String,

    /// Background behind the artwork/top region.
    #[serde(default = "default_black_color")]
    pub artwork_background: String,

    /// Background behind song title / artist / album metadata.
    #[serde(default = "default_black_color")]
    pub metadata_background: String,

    /// Background behind the analyzer / visualizer.
    #[serde(default = "default_black_color")]
    pub visualizer_background: String,
}

impl Default for DisplayColorsConfig {
    fn default() -> Self {
        Self {
            background: default_black_color(),
            artwork_background: default_black_color(),
            metadata_background: default_black_color(),
            visualizer_background: default_black_color(),
        }
    }
}

/// Named layout preset selected by `display.orientation`.
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

// ==============================================================================
// Fonts
// ==============================================================================

/// High-level font selection.
#[derive(Debug, Deserialize, Clone)]
pub struct FontsConfig {
    /// Default font theme used in fixed mode and as a manual baseline.
    pub theme: String,

    /// Font selection mode:
    /// - fixed: always use `theme`
    /// - metadata: choose a theme based on song genre/year metadata
    /// - random: planned/future option
    #[serde(default = "default_font_mode")]
    pub mode: String,

    /// Theme used when metadata mode cannot confidently choose a match.
    #[serde(default = "default_fallback_font_theme")]
    pub fallback_theme: String,
}

/// A single named font theme.
#[derive(Debug, Deserialize, Clone)]
pub struct FontTheme {
    pub title: String,
    pub body: String,
    pub title_size: u16,
    pub body_size: u16,
}

// ==============================================================================
// Visualizer
// ==============================================================================

/// Live visualizer configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct VisualizerConfig {
    pub enabled: bool,
    pub mode: String,
    pub height: u32,
    pub padding: u32,
    pub peak_hold: bool,

    // Shared visualizer timing/level controls.
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

    // Spectrum analyzer shape.
    #[serde(default = "default_spectrum_bin_count")]
    pub spectrum_bin_count: usize,

    #[serde(default = "default_spectrum_fft_size")]
    pub spectrum_fft_size: usize,

    #[serde(default = "default_spectrum_min_hz")]
    pub spectrum_min_hz: f32,

    #[serde(default = "default_spectrum_max_hz")]
    pub spectrum_max_hz: f32,

    #[serde(default = "default_spectrum_bar_gap")]
    pub spectrum_bar_gap: u32,

    // Spectrum analyzer responsiveness.
    #[serde(default = "default_spectrum_attack")]
    pub spectrum_attack: f32,

    #[serde(default = "default_spectrum_smoothing")]
    pub spectrum_smoothing: f32,

    // Spectrum analyzer scaling.
    #[serde(default = "default_spectrum_log_epsilon")]
    pub spectrum_log_epsilon: f32,

    #[serde(default = "default_spectrum_log_scale")]
    pub spectrum_log_scale: f32,

    #[serde(default = "default_spectrum_log_offset")]
    pub spectrum_log_offset: f32,

    #[serde(default = "default_spectrum_noise_floor")]
    pub spectrum_noise_floor: f32,

    #[serde(default = "default_spectrum_contrast")]
    pub spectrum_contrast: f32,

    /// Spectrum analyzer rendering options.
    #[serde(default)]
    pub spectrum: VisualizerSpectrumConfig,

    /// Spectrum peak marker options.
    #[serde(default)]
    pub peaks: VisualizerPeaksConfig,

    /// Visualizer foreground color selection.
    #[serde(default)]
    pub colors: VisualizerColorsConfig,
}

/// Spectrum analyzer rendering options.
///
/// `render_style` accepts:
/// - `full`: draw each bar from the baseline outward
/// - `top_only`: draw only the outer/top segment of each active bar
#[derive(Debug, Deserialize, Clone)]
pub struct VisualizerSpectrumConfig {
    #[serde(default = "default_spectrum_render_style")]
    pub render_style: String,

    #[serde(default = "default_top_only_height_ratio")]
    pub top_only_height_ratio: f32,
}

impl Default for VisualizerSpectrumConfig {
    fn default() -> Self {
        Self {
            render_style: default_spectrum_render_style(),
            top_only_height_ratio: default_top_only_height_ratio(),
        }
    }
}

/// Spectrum peak marker options.
#[derive(Debug, Deserialize, Clone)]
pub struct VisualizerPeaksConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_peak_hold_ms")]
    pub hold_ms: u64,

    #[serde(default = "default_peak_drop_pixels")]
    pub drop_pixels: u32,

    #[serde(default = "default_peak_color")]
    pub color: String,

    #[serde(default = "default_peak_use_bar_color")]
    pub use_bar_color: bool,
}

impl Default for VisualizerPeaksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hold_ms: default_peak_hold_ms(),
            drop_pixels: default_peak_drop_pixels(),
            color: default_peak_color(),
            use_bar_color: default_peak_use_bar_color(),
        }
    }
}

/// Configurable visualizer foreground colors.
///
/// `mode` accepts:
/// - `fixed`: use `upper` and `lower`
/// - `artwork`: derive colors from artwork, falling back to `fallback_*`
#[derive(Debug, Deserialize, Clone)]
pub struct VisualizerColorsConfig {
    #[serde(default = "default_visualizer_color_mode")]
    pub mode: String,

    #[serde(default = "default_visualizer_upper_color")]
    pub upper: String,

    #[serde(default = "default_visualizer_lower_color")]
    pub lower: String,

    #[serde(default = "default_visualizer_upper_color")]
    pub fallback_upper: String,

    #[serde(default = "default_visualizer_lower_color")]
    pub fallback_lower: String,

    #[serde(default = "default_visualizer_min_brightness")]
    pub min_brightness: u8,

    #[serde(default = "default_visualizer_min_saturation")]
    pub min_saturation: f32,

    #[serde(default = "default_visualizer_palette_size")]
    pub palette_size: usize,

    #[serde(default = "default_visualizer_hue_bucket_count")]
    pub hue_bucket_count: usize,
}

impl Default for VisualizerColorsConfig {
    fn default() -> Self {
        Self {
            mode: default_visualizer_color_mode(),
            upper: default_visualizer_upper_color(),
            lower: default_visualizer_lower_color(),
            fallback_upper: default_visualizer_upper_color(),
            fallback_lower: default_visualizer_lower_color(),
            min_brightness: default_visualizer_min_brightness(),
            min_saturation: default_visualizer_min_saturation(),
            palette_size: default_visualizer_palette_size(),
            hue_bucket_count: default_visualizer_hue_bucket_count(),
        }
    }
}

// ==============================================================================
// Defaults
// ==============================================================================

fn default_black_color() -> String {
    "#000000".to_string()
}

fn default_artwork_mode() -> String {
    "cover".to_string()
}

// Audio defaults.

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
    15_000
}

fn default_read_chunk_bytes() -> usize {
    4096
}

// Font defaults.

fn default_font_mode() -> String {
    "fixed".to_string()
}

fn default_fallback_font_theme() -> String {
    "simple".to_string()
}

// Visualizer defaults.

fn default_window_ms() -> usize {
    60
}

fn default_point_count() -> usize {
    180
}

fn default_gain() -> f32 {
    8.5
}

fn default_y_scale() -> f32 {
    0.95
}

fn default_left_y_offset() -> f32 {
    0.25
}

fn default_right_y_offset() -> f32 {
    0.75
}

fn default_visible_sample_count() -> usize {
    384
}

fn default_max_gain() -> f32 {
    32.0
}

fn default_debug_log_interval_ms() -> u64 {
    10_000
}

// Spectrum analyzer defaults.

fn default_spectrum_bin_count() -> usize {
    32
}

fn default_spectrum_fft_size() -> usize {
    256
}

fn default_spectrum_min_hz() -> f32 {
    40.0
}

fn default_spectrum_max_hz() -> f32 {
    6000.0
}

fn default_spectrum_bar_gap() -> u32 {
    2
}

fn default_spectrum_attack() -> f32 {
    0.1
}

fn default_spectrum_smoothing() -> f32 {
    0.65
}

fn default_spectrum_log_epsilon() -> f32 {
    1.0e-6
}

fn default_spectrum_log_scale() -> f32 {
    0.12
}

fn default_spectrum_log_offset() -> f32 {
    0.65
}

fn default_spectrum_noise_floor() -> f32 {
    0.0
}

fn default_spectrum_contrast() -> f32 {
    1.0
}

fn default_spectrum_render_style() -> String {
    "full".to_string()
}

fn default_top_only_height_ratio() -> f32 {
    0.35
}

fn default_peak_hold_ms() -> u64 {
    100
}

fn default_peak_drop_pixels() -> u32 {
    1
}

fn default_peak_color() -> String {
    "#FFFFFF".to_string()
}

fn default_peak_use_bar_color() -> bool {
    true
}

// Visualizer color defaults.

fn default_visualizer_color_mode() -> String {
    "fixed".to_string()
}

fn default_visualizer_upper_color() -> String {
    "#50DC78".to_string()
}

fn default_visualizer_lower_color() -> String {
    "#50A0FF".to_string()
}

fn default_visualizer_min_brightness() -> u8 {
    80
}

fn default_visualizer_min_saturation() -> f32 {
    0.25
}

fn default_visualizer_palette_size() -> usize {
    6
}

fn default_visualizer_hue_bucket_count() -> usize {
    12
}

// ==============================================================================
// Loader
// ==============================================================================

/// Loads application configuration from a TOML file.
pub fn load_config(path: &str) -> Result<AppConfig, String> {
    let raw =
        fs::read_to_string(path).map_err(|e| format!("Failed to read config {}: {e}", path))?;

    toml::from_str(&raw).map_err(|e| format!("Failed to parse config {}: {e}", path))
}
