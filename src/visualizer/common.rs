use std::collections::VecDeque;

/// Which visualizer is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisualizerMode {
    #[default]
    None,
    Oscilloscope,
    Spectrum,
    AnalogVu,
}

/// Global visualizer configuration.
/// Expand this later with colors, persistence, gain, sensitivity, etc.
#[derive(Debug, Clone)]
pub struct VisualizerConfig {
    pub mode: VisualizerMode,
    pub sample_window_size: usize,
    pub fft_size: usize,
    pub smoothing: f32,
    pub gain: f32,
    pub width: u32,
    pub height: u32,
}

impl Default for VisualizerConfig {
    fn default() -> Self {
        Self {
            mode: VisualizerMode::None,
            sample_window_size: 2048,
            fft_size: 1024,
            smoothing: 0.2,
            gain: 1.0,
            width: 320,
            height: 240,
        }
    }
}

/// A single stereo audio frame/sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioFrame {
    pub left: f32,
    pub right: f32,
}

impl AudioFrame {
    pub fn mono(&self) -> f32 {
        (self.left + self.right) * 0.5
    }
}

/// Simple stereo meter values.
#[derive(Debug, Clone, Copy, Default)]
pub struct StereoLevels {
    pub left_peak: f32,
    pub right_peak: f32,
    pub left_rms: f32,
    pub right_rms: f32,
}

/// Generic line-strip style points for rendering.
/// For now this is enough for oscilloscope and spectrum.
/// Analog VU can use these too, or later get its own richer primitives.
#[derive(Debug, Clone, Default)]
pub struct VisualizationFrame {
    pub mode: VisualizerMode,
    pub left_points: Vec<(f32, f32)>,
    pub right_points: Vec<(f32, f32)>,
    pub levels: StereoLevels,
}

impl VisualizationFrame {
    pub fn empty(mode: VisualizerMode) -> Self {
        Self {
            mode,
            left_points: Vec::new(),
            right_points: Vec::new(),
            levels: StereoLevels::default(),
        }
    }
}

/// Shared audio sample buffer for recent stereo samples.
#[derive(Debug, Clone)]
pub struct SampleBuffer {
    capacity: usize,
    samples: VecDeque<AudioFrame>,
}

impl SampleBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            samples: VecDeque::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, frame: AudioFrame) {
        if self.samples.len() >= self.capacity {
            let _ = self.samples.pop_front();
        }
        self.samples.push_back(frame);
    }

    pub fn extend<I>(&mut self, frames: I) where I: IntoIterator<Item = AudioFrame> {
        for frame in frames {
            self.push(frame);
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn recent(&self, count: usize) -> Vec<AudioFrame> {
        let take = count.min(self.samples.len());
        self.samples.iter().skip(self.samples.len().saturating_sub(take)).copied().collect()
    }
}

/// Common behavior for all visualizer modes.
pub trait Visualizer {
    fn mode(&self) -> VisualizerMode;

    fn update_config(&mut self, config: &VisualizerConfig);

    fn push_audio_frame(&mut self, frame: AudioFrame);

    fn push_audio_frames<I>(&mut self, frames: I) where I: IntoIterator<Item = AudioFrame> {
        for frame in frames {
            self.push_audio_frame(frame);
        }
    }

    fn render_frame(&mut self) -> VisualizationFrame;
}

/// Clamp to [0.0, 1.0]
pub fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Basic smoothing helper.
pub fn smooth_value(current: f32, target: f32, smoothing: f32) -> f32 {
    let s = clamp01(smoothing);
    current + (target - current) * s
}

/// Convert a signed sample (-1.0..1.0 expected) to a normalized screen Y position (0.0..1.0).
/// 0.0 is top, 1.0 is bottom.
pub fn sample_to_y(sample: f32) -> f32 {
    let clamped = sample.clamp(-1.0, 1.0);
    0.5 - clamped * 0.5
}

/// Compute peak + RMS levels from a slice of samples.
pub fn compute_stereo_levels(samples: &[AudioFrame]) -> StereoLevels {
    if samples.is_empty() {
        return StereoLevels::default();
    }

    let mut left_peak = 0.0f32;
    let mut right_peak = 0.0f32;
    let mut left_sum_sq = 0.0f32;
    let mut right_sum_sq = 0.0f32;

    for frame in samples {
        let l = frame.left.abs();
        let r = frame.right.abs();

        left_peak = left_peak.max(l);
        right_peak = right_peak.max(r);

        left_sum_sq += frame.left * frame.left;
        right_sum_sq += frame.right * frame.right;
    }

    let n = samples.len() as f32;

    StereoLevels {
        left_peak: clamp01(left_peak),
        right_peak: clamp01(right_peak),
        left_rms: clamp01((left_sum_sq / n).sqrt()),
        right_rms: clamp01((right_sum_sq / n).sqrt()),
    }
}

/// Find a zero-crossing index to stabilize waveform display.
/// Looks for a negative-to-positive crossing near the start of the slice.
pub fn find_trigger_index(samples: &[f32]) -> usize {
    if samples.len() < 2 {
        return 0;
    }

    let search_limit = samples.len().min(256);

    for i in 1..search_limit {
        if samples[i - 1] <= 0.0 && samples[i] > 0.0 {
            return i;
        }
    }

    0
}

/// Resample a mono series into screen-space points.
/// X and Y are normalized 0.0..1.0.
pub fn resample_to_points(
    samples: &[f32],
    width_points: usize,
    y_offset: f32,
    y_scale: f32
) -> Vec<(f32, f32)> {
    if samples.is_empty() || width_points == 0 {
        return Vec::new();
    }

    if width_points == 1 {
        let y = y_offset + (sample_to_y(samples[0]) - 0.5) * y_scale;
        return vec![(0.0, y)];
    }

    let last_index = samples.len().saturating_sub(1) as f32;
    let denom = (width_points - 1) as f32;

    let mut points = Vec::with_capacity(width_points);

    for x in 0..width_points {
        let t = (x as f32) / denom;
        let src_pos = t * last_index;
        let i0 = src_pos.floor() as usize;
        let i1 = (i0 + 1).min(samples.len() - 1);
        let frac = src_pos - (i0 as f32);

        let sample = samples[i0] * (1.0 - frac) + samples[i1] * frac;
        let y = y_offset + (sample_to_y(sample) - 0.5) * y_scale;

        points.push((t, y));
    }

    points
}