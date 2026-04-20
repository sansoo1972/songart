use crate::visualizer::common::{
    compute_stereo_levels,
    clamp01,
    smooth_value,
    AudioFrame,
    SampleBuffer,
    VisualizationFrame,
    Visualizer,
    VisualizerConfig,
    VisualizerMode,
};

pub struct SpectrumVisualizer {
    config: VisualizerConfig,
    buffer: SampleBuffer,
    smoothed_bins_left: Vec<f32>,
    smoothed_bins_right: Vec<f32>,
    bin_count: usize,
}

impl SpectrumVisualizer {
    pub fn new(config: VisualizerConfig) -> Self {
        let capacity = config.sample_window_size.max(config.fft_size).max(4096);
        let bin_count = 32;

        Self {
            config,
            buffer: SampleBuffer::new(capacity),
            smoothed_bins_left: vec![0.0; bin_count],
            smoothed_bins_right: vec![0.0; bin_count],
            bin_count,
        }
    }

    /// Placeholder spectrum estimator.
    /// This is NOT a true FFT yet; it just groups absolute amplitudes into pseudo-bins.
    /// Replace this later with rustfft.
    fn pseudo_spectrum(&self, samples: &[AudioFrame]) -> (Vec<f32>, Vec<f32>) {
        let mut left_bins = vec![0.0f32; self.bin_count];
        let mut right_bins = vec![0.0f32; self.bin_count];

        if samples.is_empty() {
            return (left_bins, right_bins);
        }

        let chunk_size = (samples.len() / self.bin_count).max(1);

        for (i, chunk) in samples.chunks(chunk_size).take(self.bin_count).enumerate() {
            let mut left_sum = 0.0f32;
            let mut right_sum = 0.0f32;

            for frame in chunk {
                left_sum += frame.left.abs();
                right_sum += frame.right.abs();
            }

            let denom = chunk.len().max(1) as f32;
            left_bins[i] = clamp01((left_sum / denom) * self.config.gain);
            right_bins[i] = clamp01((right_sum / denom) * self.config.gain);
        }

        (left_bins, right_bins)
    }

    fn update_smoothed_bins(&mut self, left: &[f32], right: &[f32]) {
        for i in 0..self.bin_count {
            self.smoothed_bins_left[i] = smooth_value(
                self.smoothed_bins_left[i],
                left[i],
                self.config.smoothing
            );
            self.smoothed_bins_right[i] = smooth_value(
                self.smoothed_bins_right[i],
                right[i],
                self.config.smoothing
            );
        }
    }

    fn bins_to_points(&self, bins: &[f32], y_base: f32, y_height: f32) -> Vec<(f32, f32)> {
        if bins.is_empty() {
            return Vec::new();
        }

        let count = bins.len();
        let denom = (count - 1).max(1) as f32;

        bins.iter()
            .enumerate()
            .map(|(i, value)| {
                let x = (i as f32) / denom;
                let y = y_base - *value * y_height;
                (x, y)
            })
            .collect()
    }
}

impl Visualizer for SpectrumVisualizer {
    fn mode(&self) -> VisualizerMode {
        VisualizerMode::Spectrum
    }

    fn update_config(&mut self, config: &VisualizerConfig) {
        self.config = config.clone();
    }

    fn push_audio_frame(&mut self, frame: AudioFrame) {
        self.buffer.push(frame);
    }

    fn render_frame(&mut self) -> VisualizationFrame {
        let samples = self.buffer.recent(self.config.fft_size.max(self.config.sample_window_size));
        let levels = compute_stereo_levels(&samples);

        let (left_bins, right_bins) = self.pseudo_spectrum(&samples);
        self.update_smoothed_bins(&left_bins, &right_bins);

        let left_points = self.bins_to_points(&self.smoothed_bins_left, 0.48, 0.4);
        let right_points = self.bins_to_points(&self.smoothed_bins_right, 0.98, 0.4);

        VisualizationFrame {
            mode: VisualizerMode::Spectrum,
            left_points,
            right_points,
            levels,
        }
    }
}