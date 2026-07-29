use crate::config::AppConfig;
use crate::logging::LogLevel;
use crate::visualizer::VisualizerMode;

/// Shared runtime context available across threads.
///
/// This is intentionally lightweight:
/// - immutable loaded config
/// - resolved runtime log level
#[derive(Clone)]
pub struct AppContext {
    pub config: AppConfig,
    pub log_level: LogLevel,
}

/// Simple meter state used by the live renderer.
///
/// `level` is the current normalized loudness.
/// `peak` is a decaying peak-hold value for visual emphasis.
#[derive(Clone, Debug, Default)]
pub struct MeterState {
    pub level: f32,
    pub peak: f32,
}

/// Renderable visualizer frame payload.
///
/// Points are normalized into 0.0..1.0 coordinate space and interpreted by
/// the display renderer.
#[derive(Clone, Debug, Default)]
pub struct VisualizerFrameState {
    pub left_points: Vec<(f32, f32)>,
    pub right_points: Vec<(f32, f32)>,
}

/// Shared visualizer state.
///
/// The display loop treats this as render-ready state, while live audio data
/// comes from the shared rolling audio buffer.
#[derive(Clone, Debug)]
pub struct VisualizerState {
    pub enabled: bool,
    pub mode: VisualizerMode,
    pub frame: VisualizerFrameState,
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: VisualizerMode::Oscilloscope,
            frame: VisualizerFrameState::default(),
        }
    }
}

/// Shared UI state consumed by the display renderer.
///
/// This contains metadata/artwork state and transient visualizer state.
/// Song metadata changes relatively slowly compared with live audio.
#[derive(Clone, Debug)]
pub struct SongState {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_number: String,
    pub track_total: String,
    pub disc_number: String,
    pub disc_total: String,
    pub duration: String,
    pub composer: String,
    pub lyricist: String,
    pub producer: String,
    pub released: String,
    pub genre: String,
    pub label: String,
    pub isrc: String,
    pub explicit: String,
    pub copyright: String,
    pub catalog_number: String,
    pub artwork_path: String,
    pub artwork_url: String,
    pub version: u64,
    pub meter: MeterState,
    pub visualizer: VisualizerState,
}

impl Default for SongState {
    fn default() -> Self {
        Self {
            title: "Waiting for music...".to_string(),
            artist: "No track identified yet".to_string(),
            album: "Album unknown".to_string(),
            album_artist: "Unknown".to_string(),
            track_number: "Unknown".to_string(),
            track_total: "Unknown".to_string(),
            disc_number: "Unknown".to_string(),
            disc_total: "Unknown".to_string(),
            duration: "Unknown".to_string(),
            composer: "Unknown".to_string(),
            lyricist: "Unknown".to_string(),
            producer: "Unknown".to_string(),
            released: "Unknown".to_string(),
            genre: "Unknown".to_string(),
            label: "Unknown".to_string(),
            isrc: "Unknown".to_string(),
            explicit: "Unknown".to_string(),
            copyright: "Unknown".to_string(),
            catalog_number: "Unknown".to_string(),
            artwork_path: String::new(),
            artwork_url: String::new(),
            version: 0,
            meter: MeterState::default(),
            visualizer: VisualizerState::default(),
        }
    }
}
