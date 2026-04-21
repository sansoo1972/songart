use crate::audio::{ build_pcm_oscilloscope_points, compute_pcm_rms_level };
use crate::logging::{ log_debug, log_error, log_info };
use crate::state::SongState;
use crate::{ state::AppContext, visualizer::VisualizerMode };

use std::process::Command;
use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };
use std::thread;
use std::time::Duration;

const VIS_SAMPLE_PATH: &str = "/tmp/songart-visualizer.pcm";
const VIS_RECORD_MS: u64 = 200;
const VIS_LOOP_MS: u64 = 120;

pub fn run_visualizer_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>
) {
    log_info(&ctx, "Visualizer loop started.");

    while running.load(Ordering::SeqCst) {
        if !ctx.config.visualizer.enabled {
            thread::sleep(Duration::from_millis(250));
            continue;
        }

        let record_duration = format!("{:.2}s", (VIS_RECORD_MS as f32) / 1000.0);

        let record_status = Command::new("timeout")
            .args([
                record_duration.as_str(),
                "parecord",
                "--device",
                &ctx.config.audio.device,
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "s16le",
                "--raw",
                VIS_SAMPLE_PATH,
            ])
            .status();

        match record_status {
            Ok(status) => {
                let code = status.code().unwrap_or(-1);
                if code != 124 && !status.success() {
                    log_debug(&ctx, &format!("Visualizer record status: {status}"));
                }
            }
            Err(e) => {
                log_error(&ctx, &format!("Visualizer capture failed: {e}"));
                thread::sleep(Duration::from_millis(VIS_LOOP_MS));
                continue;
            }
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        let mode_name = ctx.config.visualizer.mode.to_ascii_lowercase();

        let raw_level = compute_wav_rms_level(VIS_SAMPLE_PATH);

        let left_points = match mode_name.as_str() {
            "oscilloscope" =>
                build_wav_oscilloscope_points(VIS_SAMPLE_PATH, 120, 160, 0.25, 0.4, 1.8),
            _ => None,
        };

        let right_points = match mode_name.as_str() {
            "oscilloscope" =>
                build_wav_oscilloscope_points(VIS_SAMPLE_PATH, 120, 160, 0.75, 0.4, 1.8),
            _ => None,
        };

        {
            let mut state = shared_state.lock().unwrap();

            if let Some(raw_level) = raw_level {
                let smoothing = ctx.config.visualizer.smoothing.clamp(0.0, 1.0);

                state.meter.level = state.meter.level * smoothing + raw_level * (1.0 - smoothing);

                if ctx.config.visualizer.peak_hold {
                    if state.meter.level > state.meter.peak {
                        state.meter.peak = state.meter.level;
                    } else {
                        state.meter.peak *= 0.96;
                    }
                } else {
                    state.meter.peak = state.meter.level;
                }
            }
            let left_len = left_points
                .as_ref()
                .map(|p| p.len())
                .unwrap_or(0);
            let right_len = right_points
                .as_ref()
                .map(|p| p.len())
                .unwrap_or(0);

            let left_head = left_points
                .as_ref()
                .and_then(|p| p.get(0))
                .copied()
                .unwrap_or((0.0, 0.0));

            let left_mid = left_points
                .as_ref()
                .and_then(|p| p.get(left_len / 2))
                .copied()
                .unwrap_or((0.0, 0.0));

            log_debug(
                &ctx,
                &format!(
                    "vis update: level={:.3?} left_len={} right_len={} left_head=({:.3},{:.3}) left_mid=({:.3},{:.3})",
                    raw_level,
                    left_len,
                    right_len,
                    left_head.0,
                    left_head.1,
                    left_mid.0,
                    left_mid.1
                )
            );
            state.visualizer.enabled = true;
            state.visualizer.mode = match mode_name.as_str() {
                "oscilloscope" => VisualizerMode::Oscilloscope,
                "spectrum" => VisualizerMode::Spectrum,
                "analog_vu" => VisualizerMode::AnalogVu,
                _ => VisualizerMode::None,
            };

            state.visualizer.frame.left_points = left_points.unwrap_or_default();
            state.visualizer.frame.right_points = right_points.unwrap_or_default();
        }

        thread::sleep(Duration::from_millis(VIS_LOOP_MS));
    }

    log_info(&ctx, "Visualizer loop stopped.");
}
