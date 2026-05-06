use rustfft::{ num_complex::Complex, FftPlanner };

/// Computes normalized spectrum magnitudes using a real FFT.
///
/// This function:
/// - takes the most recent `fft_size` samples
/// - applies a Hann window
/// - runs a forward FFT
/// - groups the FFT output into log-spaced frequency bands
/// - normalizes the result for display
pub fn compute_spectrum_bins(
    samples: &[f32],
    sample_rate: usize,
    fft_size: usize,
    bin_count: usize,
    min_hz: f32,
    max_hz: f32,
    gain: f32,
    _max_gain: f32
) -> Vec<f32> {
    if samples.is_empty() || fft_size < 64 || bin_count == 0 {
        return vec![0.0; bin_count];
    }

    let take = fft_size.min(samples.len());
    let start = samples.len().saturating_sub(take);
    let slice = &samples[start..];

    let mut input: Vec<Complex<f32>> = Vec::with_capacity(fft_size);

    for i in 0..fft_size {
        let sample = if i < slice.len() { slice[i] } else { 0.0 };

        let phase = (i as f32) / (fft_size.saturating_sub(1).max(1) as f32);
        let hann = 0.5 - 0.5 * (std::f32::consts::TAU * phase).cos();

        input.push(Complex::new(sample * hann, 0.0));
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    fft.process(&mut input);

    let nyquist = (sample_rate as f32) / 2.0;
    let min_hz = min_hz.clamp(1.0, nyquist);
    let max_hz = max_hz.clamp(min_hz + 1.0, nyquist);

    let mut out = vec![0.0f32; bin_count];

    for band in 0..bin_count {
        let t0 = (band as f32) / (bin_count as f32);
        let t1 = ((band + 1) as f32) / (bin_count as f32);

        let f0 = min_hz * (max_hz / min_hz).powf(t0);
        let f1 = min_hz * (max_hz / min_hz).powf(t1);

        let mut i0 = ((f0 / (sample_rate as f32)) * (fft_size as f32)) as usize;
        let mut i1 = ((f1 / (sample_rate as f32)) * (fft_size as f32)) as usize;

        i0 = i0.clamp(1, fft_size / 2);
        i1 = i1.clamp(i0 + 1, fft_size / 2);

        let mut energy = 0.0f32;
        let mut count = 0usize;

        for bin in &input[i0..i1] {
            energy += bin.norm();
            count += 1;
        }

        if count > 0 {
            out[band] = energy / (count as f32);
        }
    }

    /*
    let peak = out.iter().copied().fold(0.0f32, f32::max).max(0.0001);
    let normalize = (gain / peak).clamp(1.0, max_gain.max(1.0));
    */
    let scale = gain.max(0.0);

    for value in &mut out {
        let db_like = (*value + 1.0e-6).log10() * 0.25 + 1.0;
        *value = (db_like * scale).clamp(0.0, 1.0);
    }

    out
}
