/// Which visualizer is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualizerMode {
    None,
    Oscilloscope,
    Spectrum,
    AnalogVu,
}