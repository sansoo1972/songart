use std::fs;

const WAV_HEADER_SIZE: usize = 44;
const DEFAULT_SAMPLE_RATE: usize = 16_000;

/// Computes a simple RMS level from the tail end of the recorded WAV file.
///
/// Assumptions:
/// - 16-bit little-endian PCM
/// - mono
/// - standard 44-byte WAV header
pub fn compute_wav_rms_level(path: &str) -> Option<f32> {
    let samples = read_wav_tail_mono_samples(path, 200)?;
    if samples.is_empty() {
        return None;
    }

    let mut sum = 0.0f64;
    for sample in &samples {
        let s = *sample as f64;
        sum += s * s;
    }

    let rms = (sum / (samples.len() as f64)).sqrt() as f32;

    // Boost for UI readability.
    Some((rms * 6.0).clamp(0.0, 1.0))
}

/// Reads the last `tail_ms` milliseconds of a WAV file as normalized mono samples.
/// Returns values in the range -1.0..1.0.
///
/// Assumptions:
/// - 16-bit little-endian PCM
/// - mono
/// - standard 44-byte WAV header
/// - 16kHz sample rate
pub fn read_wav_tail_mono_samples(path: &str, tail_ms: usize) -> Option<Vec<f32>> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() <= WAV_HEADER_SIZE {
        return None;
    }

    let pcm = &bytes[WAV_HEADER_SIZE..];

    // 16-bit mono PCM => 2 bytes per sample
    let bytes_per_second = DEFAULT_SAMPLE_RATE * 2;
    let tail_bytes = ((bytes_per_second as f32) * ((tail_ms as f32) / 1000.0)) as usize;
    let tail_len = tail_bytes.min(pcm.len());

    let pcm_tail = &pcm[pcm.len().saturating_sub(tail_len)..];

    let mut samples = Vec::with_capacity(pcm_tail.len() / 2);

    for chunk in pcm_tail.chunks_exact(2) {
        let sample = (i16::from_le_bytes([chunk[0], chunk[1]]) as f32) / (i16::MAX as f32);
        samples.push(sample);
    }

    if samples.is_empty() {
        return None;
    }

    Some(samples)
}

/// Builds normalized oscilloscope points from the tail of a WAV file.
///
/// Returns points in normalized coordinate space:
/// - x: 0.0..1.0
/// - y: 0.0..1.0
///
/// `y_offset` is the vertical center of the waveform.
/// `y_scale` controls waveform height.
///
/// Examples:
/// - upper channel: y_offset=0.25, y_scale=0.40
/// - lower channel: y_offset=0.75, y_scale=0.40
pub fn build_wav_oscilloscope_points(
    path: &str,
    tail_ms: usize,
    width_points: usize,
    y_offset: f32,
    y_scale: f32,
    gain: f32
) -> Option<Vec<(f32, f32)>> {
    let mut samples = read_wav_tail_mono_samples(path, tail_ms)?;
    if samples.is_empty() || width_points < 2 {
        return None;
    }

    for sample in &mut samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }

    let visible = &samples;

    use std::time::{ SystemTime, UNIX_EPOCH };

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as usize;

    let window_size = samples.len().min(800); // visible chunk
    let max_offset = samples.len().saturating_sub(window_size);

    let offset = if max_offset > 0 {
        (now / 10) % max_offset // speed control here
    } else {
        0
    };

    let visible = &samples[offset..offset + window_size];

    if visible.len() < 2 {
        return None;
    }

    Some(resample_to_points(visible, width_points, y_offset, y_scale))
}

fn find_trigger_index(samples: &[f32]) -> usize {
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

fn resample_to_points(
    samples: &[f32],
    width_points: usize,
    y_offset: f32,
    y_scale: f32
) -> Vec<(f32, f32)> {
    if samples.is_empty() || width_points == 0 {
        return Vec::new();
    }

    if width_points == 1 {
        return vec![(0.0, sample_to_y(samples[0], y_offset, y_scale))];
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
        let y = sample_to_y(sample, y_offset, y_scale);
        points.push((t, y));
    }

    points
}

fn sample_to_y(sample: f32, y_offset: f32, y_scale: f32) -> f32 {
    let clamped = sample.clamp(-1.0, 1.0);
    (y_offset - clamped * 0.5 * y_scale).clamp(0.0, 1.0)
}
pub fn compute_pcm_rms_level(path: &str) -> Option<f32> {
    let samples = read_pcm_samples(path)?;
    if samples.is_empty() {
        return None;
    }

    let mut sum = 0.0f64;
    for sample in &samples {
        let s = *sample as f64;
        sum += s * s;
    }

    let rms = (sum / (samples.len() as f64)).sqrt() as f32;
    Some((rms * 6.0).clamp(0.0, 1.0))
}

pub fn build_pcm_oscilloscope_points(
    path: &str,
    width_points: usize,
    y_offset: f32,
    y_scale: f32,
    gain: f32
) -> Option<Vec<(f32, f32)>> {
    let mut samples = read_pcm_samples(path)?;
    if samples.is_empty() || width_points < 2 {
        return None;
    }

    for sample in &mut samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }

    Some(resample_to_points(&samples, width_points, y_offset, y_scale))
}

fn read_pcm_samples(path: &str) -> Option<Vec<f32>> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 4 {
        return None;
    }

    let mut samples = Vec::with_capacity(bytes.len() / 2);

    for chunk in bytes.chunks_exact(2) {
        let sample = (i16::from_le_bytes([chunk[0], chunk[1]]) as f32) / (i16::MAX as f32);
        samples.push(sample);
    }

    Some(samples)
}