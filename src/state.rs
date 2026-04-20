use crate::config::AppConfig;
use crate::logging::LogLevel;
use crate::visualizer::VisualizerMode;

/// Shared runtime context.
#[derive(Clone)]
pub struct AppContext {
    pub config: AppConfig,
    pub log_level: LogLevel,
}

/// Meter state for the digital VU meter.
#[derive(Clone, Debug, Default)]
pub struct MeterState {
    pub level: f32,
    pub peak: f32,
}

/// Lightweight renderable visualizer snapshot.
/// Normalized coordinates: x and y are expected in 0.0..1.0 space.
#[derive(Clone, Debug, Default)]
pub struct VisualizerFrameState {
    pub left_points: Vec<(f32, f32)>,
    pub right_points: Vec<(f32, f32)>,
}

/// Shared visualizer state.
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

/// Shared UI state consumed by the SDL renderer.
#[derive(Clone, Debug)]
pub struct SongState {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: String,
    pub composer: String,
    pub released: String,
    pub genre: String,
    pub label: String,
    pub notes: String,
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
            track_number: "Unknown".to_string(),
            composer: "Unknown".to_string(),
            released: "Unknown".to_string(),
            genre: "Unknown".to_string(),
            label: "Unknown".to_string(),
            notes: "Listening for audio input".to_string(),
            artwork_path: String::new(),
            artwork_url: String::new(),
            version: 0,
            meter: MeterState::default(),
            visualizer: VisualizerState::default(),
        }
    }
}