use std::fs;

/// Computes a simple RMS level from the tail end of the recorded WAV file.
///
/// This makes the VU meter more responsive than averaging the entire clip.
/// Assumes:
/// - 16-bit little-endian PCM
/// - standard 44-byte WAV header
pub fn compute_wav_rms_level(path: &str) -> Option<f32> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() <= 44 {
        return None;
    }

    let pcm = &bytes[44..];

    // Last ~200 ms for 16kHz mono 16-bit PCM:
    // 16000 samples/sec * 2 bytes/sample * 0.20 sec ≈ 6400 bytes
    let tail_len = 6400usize.min(pcm.len());
    let pcm_tail = &pcm[pcm.len() - tail_len..];

    let mut sum = 0.0f64;
    let mut count = 0usize;

    for chunk in pcm_tail.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f64 / i16::MAX as f64;
        sum += sample * sample;
        count += 1;
    }

    if count == 0 {
        return None;
    }

    let rms = (sum / count as f64).sqrt() as f32;

    // Boost for UI readability.
    Some((rms * 6.0).clamp(0.0, 1.0))
}