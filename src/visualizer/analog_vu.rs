use crate::visualizer::common::{
    clamp01,
    compute_stereo_levels,
    smooth_value,
    AudioFrame,
    SampleBuffer,
    StereoLevels,
    VisualizationFrame,
    Visualizer,
    VisualizerConfig,
    VisualizerMode,
};

pub struct AnalogVuVisualizer {
    config: VisualizerConfig,
    buffer: SampleBuffer,
    displayed_levels: StereoLevels,
}

impl AnalogVuVisualizer {
    pub fn new(config: VisualizerConfig) -> Self {
        let capacity = config.sample_window_size.max(2048);

        Self {
            config,
            buffer: SampleBuffer::new(capacity),
            displayed_levels: StereoLevels::default(),
        }
    }

    fn update_ballistics(&mut self, target: StereoLevels) {
        // Faster rise, slower fall would be better later.
        // This first pass uses one smoothing value for simplicity.
        self.displayed_levels.left_peak = smooth_value(
            self.displayed_levels.left_peak,
            target.left_peak,
            self.config.smoothing
        );
        self.displayed_levels.right_peak = smooth_value(
            self.displayed_levels.right_peak,
            target.right_peak,
            self.config.smoothing
        );
        self.displayed_levels.left_rms = smooth_value(
            self.displayed_levels.left_rms,
            target.left_rms,
            self.config.smoothing
        );
        self.displayed_levels.right_rms = smooth_value(
            self.displayed_levels.right_rms,
            target.right_rms,
            self.config.smoothing
        );
    }

    fn meter_arc_points(
        &self,
        center_x: f32,
        center_y: f32,
        radius: f32,
        value: f32
    ) -> Vec<(f32, f32)> {
        let v = clamp01(value);

        // Needle sweep from about -120° to -30° in normalized radians.
        let start_deg = -120.0f32;
        let end_deg = -30.0f32;
        let angle_deg = start_deg + (end_deg - start_deg) * v;
        let angle = angle_deg.to_radians();

        let tip_x = center_x + radius * angle.cos();
        let tip_y = center_y + radius * angle.sin();

        vec![(center_x, center_y), (tip_x, tip_y)]
    }
}

impl Visualizer for AnalogVuVisualizer {
    fn mode(&self) -> VisualizerMode {
        VisualizerMode::AnalogVu
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
        self.update_ballistics(levels);

        // For analog VU, use RMS as the primary displayed value.
        let left_points = self.meter_arc_points(0.25, 0.8, 0.22, self.displayed_levels.left_rms);
        let right_points = self.meter_arc_points(0.75, 0.8, 0.22, self.displayed_levels.right_rms);

        VisualizationFrame {
            mode: VisualizerMode::AnalogVu,
            left_points,
            right_points,
            levels: self.displayed_levels,
        }
    }
}