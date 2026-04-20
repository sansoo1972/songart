use crate::visualizer::common::{
    compute_stereo_levels,
    find_trigger_index,
    resample_to_points,
    AudioFrame,
    SampleBuffer,
    VisualizationFrame,
    Visualizer,
    VisualizerConfig,
    VisualizerMode,
};

pub struct OscilloscopeVisualizer {
    config: VisualizerConfig,
    buffer: SampleBuffer,
}

impl OscilloscopeVisualizer {
    pub fn new(config: VisualizerConfig) -> Self {
        let capacity = config.sample_window_size.max(4096);
        Self {
            buffer: SampleBuffer::new(capacity),
            config,
        }
    }

    fn build_channel_points(&self, samples: &[AudioFrame]) -> (Vec<(f32, f32)>, Vec<(f32, f32)>) {
        if samples.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let left: Vec<f32> = samples
            .iter()
            .map(|f| f.left * self.config.gain)
            .collect();
        let right: Vec<f32> = samples
            .iter()
            .map(|f| f.right * self.config.gain)
            .collect();

        let trigger = find_trigger_index(&left);
        let visible_len = left.len().saturating_sub(trigger);

        if visible_len == 0 {
            return (Vec::new(), Vec::new());
        }

        let left_visible = &left[trigger..];
        let right_visible = &right[trigger..];

        let width_points = self.config.width.max(2) as usize;

        // Left channel in upper half, right in lower half.
        let left_points = resample_to_points(left_visible, width_points, 0.25, 0.45);
        let right_points = resample_to_points(right_visible, width_points, 0.75, 0.45);

        (left_points, right_points)
    }
}

impl Visualizer for OscilloscopeVisualizer {
    fn mode(&self) -> VisualizerMode {
        VisualizerMode::Oscilloscope
    }

    fn update_config(&mut self, config: &VisualizerConfig) {
        self.config = config.clone();
    }

    fn push_audio_frame(&mut self, frame: AudioFrame) {
        self.buffer.push(frame);
    }

    fn render_frame(&mut self) -> VisualizationFrame {
        let samples = self.buffer.recent(self.config.sample_window_size);
        let levels = compute_stereo_levels(&samples);
        let (left_points, right_points) = self.build_channel_points(&samples);

        VisualizationFrame {
            mode: VisualizerMode::Oscilloscope,
            left_points,
            right_points,
            levels,
        }
    }
}