use crate::logging::{ log_debug, log_error, log_info };
use crate::state::AppContext;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{ Read, Write };
use std::process::{ Command, Stdio };
use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };

/// Shared rolling mono audio buffer used by both the live visualizer and the
/// slower recognition pipeline.
///
/// The buffer stores normalized mono samples in the range -1.0..1.0.
#[derive(Debug)]
pub struct SharedAudioBuffer {
    samples: VecDeque<f32>,
    max_samples: usize,
    sample_rate: usize,
}

impl SharedAudioBuffer {
    /// Creates a new rolling buffer sized for `max_seconds` worth of samples.
    pub fn new(max_seconds: usize, sample_rate: usize) -> Self {
        let max_samples = sample_rate * max_seconds;
        Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
            sample_rate,
        }
    }

    /// Appends newly decoded samples, trimming the oldest data when the buffer
    /// reaches capacity.
    pub fn push_samples(&mut self, new_samples: &[f32]) {
        for sample in new_samples {
            if self.samples.len() >= self.max_samples {
                let _ = self.samples.pop_front();
            }
            self.samples.push_back(*sample);
        }
    }

    /// Returns the most recent `count` samples from the rolling buffer.
    pub fn recent_samples(&self, count: usize) -> Vec<f32> {
        let take = count.min(self.samples.len());
        self.samples.iter().skip(self.samples.len().saturating_sub(take)).copied().collect()
    }

    /// Returns the most recent `ms` milliseconds of audio.
    pub fn recent_ms(&self, ms: usize) -> Vec<f32> {
        let count = (self.sample_rate * ms) / 1000;
        self.recent_samples(count)
    }

    /// Returns the number of currently buffered samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Returns true when no samples have been captured yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Returns the configured sample rate for this buffer.
    pub fn sample_rate(&self) -> usize {
        self.sample_rate
    }
}

/// Creates the shared live audio buffer using values from configuration.
pub fn create_shared_audio_buffer(ctx: &AppContext) -> Arc<Mutex<SharedAudioBuffer>> {
    Arc::new(
        Mutex::new(
            SharedAudioBuffer::new(ctx.config.audio.buffer_seconds, ctx.config.audio.sample_rate)
        )
    )
}

/// Continuous audio capture loop using a single long-lived `parec` process.
///
/// This is the single source of truth for live audio inside the application.
/// Visualizer rendering and recognition snapshots both consume from the same
/// rolling in-memory buffer to avoid competing parallel recorders.
pub fn run_audio_capture_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_audio: Arc<Mutex<SharedAudioBuffer>>
) {
    log_info(&ctx, "Audio capture loop started.");

    let sample_rate_arg = ctx.config.audio.sample_rate.to_string();
    let channels_arg = ctx.config.audio.channels.to_string();

    let mut child = match
        Command::new("parec")
            .args([
                "--device",
                &ctx.config.audio.device,
                "--rate",
                &sample_rate_arg,
                "--channels",
                &channels_arg,
                "--format",
                "s16le",
                "--raw",
            ])
            .stdout(Stdio::piped())
            .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log_error(&ctx, &format!("Failed to start parec: {e}"));
            return;
        }
    };

    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            log_error(&ctx, "parec stdout was not available.");
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
    };

    let mut buf = vec![0u8; ctx.config.audio.read_chunk_bytes.max(512)];
    let bytes_per_sample = 2usize;

    while running.load(Ordering::SeqCst) {
        match stdout.read(&mut buf) {
            Ok(0) => {
                log_debug(&ctx, "parec returned EOF.");
                break;
            }
            Ok(n) => {
                let mut decoded = Vec::with_capacity(n / bytes_per_sample);

                for chunk in buf[..n].chunks_exact(2) {
                    let sample =
                        (i16::from_le_bytes([chunk[0], chunk[1]]) as f32) / (i16::MAX as f32);
                    decoded.push(sample);
                }

                if !decoded.is_empty() {
                    let mut audio = shared_audio.lock().unwrap();
                    audio.push_samples(&decoded);
                }
            }
            Err(e) => {
                log_error(&ctx, &format!("Error reading from parec: {e}"));
                break;
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    log_info(&ctx, "Audio capture loop stopped.");
}

/// Computes RMS loudness from a mono sample slice.
///
/// The result is normalized and lightly boosted for UI readability.
pub fn compute_rms(samples: &[f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }

    let mut sum = 0.0f64;
    for sample in samples {
        let s = *sample as f64;
        sum += s * s;
    }

    let rms = (sum / (samples.len() as f64)).sqrt() as f32;

    Some((rms * 6.0).clamp(0.0, 1.0))
}

/// Builds normalized oscilloscope points from a mono sample slice.
///
/// The function intentionally:
/// - takes only the most recent `visible_sample_count` samples
/// - applies local auto-normalization based on the visible peak
/// - rescales into normalized screen-space points
///
/// This makes small signal changes easier to see on screen.
pub fn build_oscilloscope_points(
    samples: &[f32],
    width_points: usize,
    y_offset: f32,
    y_scale: f32,
    gain: f32,
    visible_sample_count: usize,
    max_gain: f32
) -> Vec<(f32, f32)> {
    if samples.is_empty() || width_points < 2 {
        return Vec::new();
    }

    let visible_len = samples.len().min(visible_sample_count.max(32)).max(32);
    let start = samples.len().saturating_sub(visible_len);
    let visible = &samples[start..];

    let peak = visible
        .iter()
        .fold(0.0f32, |acc, s| acc.max(s.abs()))
        .max(0.01);

    let normalized_gain = (gain / peak).clamp(1.0, max_gain.max(1.0));

    let amplified: Vec<f32> = visible
        .iter()
        .map(|s| (*s * normalized_gain).clamp(-1.0, 1.0))
        .collect();

    resample_to_points(&amplified, width_points, y_offset, y_scale)
}

/// Writes a mono PCM WAV snapshot for SongRec from the latest buffered audio.
///
/// The WAV format is:
/// - PCM
/// - 16-bit
/// - mono
/// - configured sample rate
pub fn write_wav_snapshot(
    path: &str,
    samples: &[f32],
    sample_rate: usize,
    channels: usize
) -> std::io::Result<()> {
    let mut file = File::create(path)?;

    let bytes_per_sample = 2usize;
    let data_len = (samples.len() * bytes_per_sample) as u32;
    let riff_len = 36 + data_len;
    let byte_rate = (sample_rate * channels * bytes_per_sample) as u32;
    let block_align = (channels * bytes_per_sample) as u16;

    file.write_all(b"RIFF")?;
    file.write_all(&riff_len.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    file.write_all(b"fmt ")?;
    file.write_all(&(16u32).to_le_bytes())?;
    file.write_all(&(1u16).to_le_bytes())?;
    file.write_all(&(channels as u16).to_le_bytes())?;
    file.write_all(&(sample_rate as u32).to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&(16u16).to_le_bytes())?;

    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;

    for sample in samples {
        let s = (sample.clamp(-1.0, 1.0) * (i16::MAX as f32)) as i16;
        file.write_all(&s.to_le_bytes())?;
    }

    Ok(())
}

/// Resamples a slice into normalized X/Y points for line rendering.
fn resample_to_points(
    samples: &[f32],
    width_points: usize,
    y_offset: f32,
    y_scale: f32
) -> Vec<(f32, f32)> {
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

/// Converts a normalized sample to normalized vertical screen position.
fn sample_to_y(sample: f32, y_offset: f32, y_scale: f32) -> f32 {
    let clamped = sample.clamp(-1.0, 1.0);
    (y_offset - clamped * 0.5 * y_scale).clamp(0.0, 1.0)
}