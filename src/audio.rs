use crate::logging::{ log_error, log_info };
use crate::state::AppContext;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{ Read, Write };
use std::process::{ Command, Stdio };
use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };

pub const SAMPLE_RATE: usize = 16_000;
const CHANNELS: usize = 1;
const BYTES_PER_SAMPLE: usize = 2;
const MAX_BUFFER_SECONDS: usize = 20;

/// Shared rolling mono audio buffer.
#[derive(Debug)]
pub struct SharedAudioBuffer {
    samples: VecDeque<f32>,
    max_samples: usize,
}

impl SharedAudioBuffer {
    pub fn new(max_seconds: usize) -> Self {
        let max_samples = SAMPLE_RATE * max_seconds;
        Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
        }
    }

    pub fn push_samples(&mut self, new_samples: &[f32]) {
        for sample in new_samples {
            if self.samples.len() >= self.max_samples {
                let _ = self.samples.pop_front();
            }
            self.samples.push_back(*sample);
        }
    }

    pub fn recent_samples(&self, count: usize) -> Vec<f32> {
        let take = count.min(self.samples.len());
        self.samples.iter().skip(self.samples.len().saturating_sub(take)).copied().collect()
    }

    pub fn recent_ms(&self, ms: usize) -> Vec<f32> {
        let count = (SAMPLE_RATE * ms) / 1000;
        self.recent_samples(count)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

pub fn create_shared_audio_buffer() -> Arc<Mutex<SharedAudioBuffer>> {
    Arc::new(Mutex::new(SharedAudioBuffer::new(MAX_BUFFER_SECONDS)))
}

/// Continuous audio capture loop using a single `parec` process.
/// This is the single source of truth for live audio.
pub fn run_audio_capture_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_audio: Arc<Mutex<SharedAudioBuffer>>
) {
    log_info(&ctx, "Audio capture loop started.");

    let mut child = match
        Command::new("parec")
            .args([
                "--device",
                &ctx.config.audio.device,
                "--rate",
                "16000",
                "--channels",
                "1",
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

    let mut buf = [0u8; 4096];

    while running.load(Ordering::SeqCst) {
        match stdout.read(&mut buf) {
            Ok(0) => {
                break;
            }
            Ok(n) => {
                let mut decoded = Vec::with_capacity(n / BYTES_PER_SAMPLE);

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

    // Boost for UI readability.
    Some((rms * 6.0).clamp(0.0, 1.0))
}

pub fn build_oscilloscope_points(
    samples: &[f32],
    width_points: usize,
    y_offset: f32,
    y_scale: f32,
    gain: f32
) -> Vec<(f32, f32)> {
    if samples.is_empty() || width_points < 2 {
        return Vec::new();
    }

    let amplified: Vec<f32> = samples
        .iter()
        .map(|s| (*s * gain).clamp(-1.0, 1.0))
        .collect();

    resample_to_points(&amplified, width_points, y_offset, y_scale)
}

/// Writes a mono 16kHz 16-bit PCM WAV snapshot for SongRec.
pub fn write_wav_snapshot(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let mut file = File::create(path)?;

    let data_len = (samples.len() * 2) as u32;
    let riff_len = 36 + data_len;
    let byte_rate = (SAMPLE_RATE * CHANNELS * BYTES_PER_SAMPLE) as u32;
    let block_align = (CHANNELS * BYTES_PER_SAMPLE) as u16;

    file.write_all(b"RIFF")?;
    file.write_all(&riff_len.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    file.write_all(b"fmt ")?;
    file.write_all(&(16u32).to_le_bytes())?;
    file.write_all(&(1u16).to_le_bytes())?; // PCM
    file.write_all(&(CHANNELS as u16).to_le_bytes())?;
    file.write_all(&(SAMPLE_RATE as u32).to_le_bytes())?;
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

fn sample_to_y(sample: f32, y_offset: f32, y_scale: f32) -> f32 {
    let clamped = sample.clamp(-1.0, 1.0);
    (y_offset - clamped * 0.5 * y_scale).clamp(0.0, 1.0)
}