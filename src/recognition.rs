use crate::audio::{ build_wav_oscilloscope_points, compute_wav_rms_level };
use crate::logging::{ log_blank, log_debug, log_error, log_info };
use crate::state::{ AppContext, SongState };

use serde_json::Value;
use std::fs;
use std::process::Command;
use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };
use std::thread;
use std::time::Duration;

/// Pulls a metadata value out of the nested SongRec/Shazam JSON sections by title.
fn metadata_value(json: &Value, wanted_title: &str) -> Option<String> {
    let sections = json["track"]["sections"].as_array()?;

    for section in sections {
        let metadata = section["metadata"].as_array()?;
        for item in metadata {
            let title = item["title"].as_str().unwrap_or("");
            if title.eq_ignore_ascii_case(wanted_title) {
                let text = item["text"].as_str().unwrap_or("").trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    None
}

fn extract_album(json: &Value) -> String {
    metadata_value(json, "Album").unwrap_or_else(|| "Unknown".to_string())
}

fn extract_label(json: &Value) -> String {
    metadata_value(json, "Label").unwrap_or_else(|| "Unknown".to_string())
}

fn extract_released(json: &Value) -> String {
    metadata_value(json, "Released").unwrap_or_else(|| "Unknown".to_string())
}

fn extract_composer(json: &Value) -> String {
    metadata_value(json, "Composer")
        .or_else(|| metadata_value(json, "Writers"))
        .or_else(|| metadata_value(json, "Writer"))
        .unwrap_or_else(|| "Unknown".to_string())
}

fn extract_track_number(json: &Value) -> String {
    metadata_value(json, "Track")
        .or_else(|| metadata_value(json, "Track Number"))
        .unwrap_or_else(|| "Unknown".to_string())
}

fn extract_genre(json: &Value) -> String {
    json["track"]["genres"]["primary"].as_str().unwrap_or("Unknown").to_string()
}

fn extract_isrc(json: &Value) -> String {
    json["track"]["isrc"].as_str().unwrap_or("Unknown").to_string()
}

fn extract_notes(json: &Value) -> String {
    let mut bits = Vec::new();

    let genre = extract_genre(json);
    if genre != "Unknown" {
        bits.push(format!("Genre: {genre}"));
    }

    let label = extract_label(json);
    if label != "Unknown" {
        bits.push(format!("Label: {label}"));
    }

    let isrc = extract_isrc(json);
    if isrc != "Unknown" {
        bits.push(format!("ISRC: {isrc}"));
    }

    if bits.is_empty() {
        "None".to_string()
    } else {
        bits.join(" | ")
    }
}

/// Builds an ordered list of artwork URL candidates.
fn artwork_candidates(url: &str) -> Vec<String> {
    let mut out = Vec::new();

    if url.contains("mzstatic.com") {
        let replacements = [
            ("400x400cc.jpg", "3000x3000bb.jpg"),
            ("400x400cc.jpg", "2000x2000bb.jpg"),
            ("400x400cc.jpg", "1400x1400bb.jpg"),
            ("400x400cc.jpg", "1200x1200bb.jpg"),
            ("400x400cc.jpg", "800x800bb.jpg"),
            ("400x400cc.jpg", "600x600bb.jpg"),
            ("400x400cc.jpg", "400x400bb.jpg"),
            ("400x400cc.jpg", "3000x3000cc.jpg"),
            ("400x400cc.jpg", "1400x1400cc.jpg"),
            ("400x400cc.jpg", "1200x1200cc.jpg"),
            ("400x400cc.jpg", "800x800cc.jpg"),
        ];

        for (from, to) in replacements {
            if url.contains(from) {
                out.push(url.replace(from, to));
            }
        }
    }

    out.push(url.to_string());
    out.dedup();
    out
}

/// Picks the first available seed artwork URL from the JSON response.
fn pick_artwork_url(json: &Value) -> Option<String> {
    let mut base_urls = Vec::new();

    if let Some(url) = json["track"]["images"]["coverarthq"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["coverart"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["background"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if base_urls.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    for url in base_urls {
        candidates.extend(artwork_candidates(&url));
    }

    candidates.dedup();
    candidates.into_iter().next()
}

/// Downloads the best available artwork and writes it atomically.
fn download_best_artwork(
    ctx: &AppContext,
    json: &Value,
    output_path: &str
) -> Result<String, String> {
    let mut base_urls = Vec::new();

    if let Some(url) = json["track"]["images"]["coverarthq"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["coverart"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["background"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if base_urls.is_empty() {
        return Err("No artwork URL found in JSON".to_string());
    }

    let client = reqwest::blocking::Client
        ::builder()
        .user_agent("songart/0.1")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let mut candidates = Vec::new();
    for url in base_urls {
        candidates.extend(artwork_candidates(&url));
    }

    candidates.dedup();

    for candidate in candidates {
        log_debug(ctx, &format!("Trying artwork: {candidate}"));

        let resp = match client.get(&candidate).send() {
            Ok(r) => r,
            Err(e) => {
                log_debug(ctx, &format!("Download failed: {e}"));
                continue;
            }
        };

        if !resp.status().is_success() {
            log_debug(ctx, &format!("HTTP status {} for {}", resp.status(), candidate));
            continue;
        }

        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => {
                log_debug(ctx, &format!("Failed reading bytes: {e}"));
                continue;
            }
        };

        if bytes.len() < 10_000 {
            log_debug(ctx, &format!("Rejected tiny image ({} bytes): {}", bytes.len(), candidate));
            continue;
        }

        let tmp_path = format!("{output_path}.tmp");

        fs
            ::write(&tmp_path, &bytes)
            .map_err(|e| format!("Failed to save temp artwork to {}: {e}", tmp_path))?;

        fs
            ::rename(&tmp_path, output_path)
            .map_err(|e| format!("Failed to rename temp artwork to {}: {e}", output_path))?;

        return Ok(candidate);
    }

    Err("No usable artwork URL succeeded".to_string())
}

pub fn run_recognition_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>
) {
    let mut last_track = String::new();
    let mut last_artwork_url = String::new();

    log_info(&ctx, &format!("Log file: {}", ctx.config.logging.file));
    log_info(&ctx, "Recognition loop started.");

    while running.load(Ordering::SeqCst) {
        log_info(&ctx, "Listening...");

        let record_duration = format!("{}s", ctx.config.audio.record_seconds);

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
                &ctx.config.audio.sample_wav,
            ])
            .status();

        match record_status {
            Ok(status) => {
                log_debug(&ctx, &format!("Record command exit status: {status}"));
            }
            Err(e) => {
                log_error(&ctx, &format!("Failed to record sample audio: {e}"));
                thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
                continue;
            }
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        // Update visualizer state from the most recently recorded audio sample.
        if ctx.config.visualizer.enabled {
            let mode_name = ctx.config.visualizer.mode.to_ascii_lowercase();

            let raw_level = compute_wav_rms_level(&ctx.config.audio.sample_wav);

            let left_points = if mode_name == "oscilloscope" {
                build_wav_oscilloscope_points(
                    &ctx.config.audio.sample_wav,
                    120, // ms of tail audio to inspect
                    160, // number of points across the display
                    0.25, // upper channel center
                    0.4, // waveform height
                    1.5 // gain
                )
            } else {
                None
            };

            let right_points = if mode_name == "oscilloscope" {
                build_wav_oscilloscope_points(
                    &ctx.config.audio.sample_wav,
                    120,
                    160,
                    0.75, // lower channel center
                    0.4,
                    1.5
                )
            } else {
                None
            };

            log_debug(
                &ctx,
                &format!(
                    "oscilloscope points: left={}, right={}",
                    left_points
                        .as_ref()
                        .map(|p| p.len())
                        .unwrap_or(0),
                    right_points
                        .as_ref()
                        .map(|p| p.len())
                        .unwrap_or(0)
                )
            );

            log_debug(
                &ctx,
                &format!(
                    "visualizer mode={}, left_points={}, right_points={}",
                    mode_name,
                    left_points
                        .as_ref()
                        .map(|p| p.len())
                        .unwrap_or(0),
                    right_points
                        .as_ref()
                        .map(|p| p.len())
                        .unwrap_or(0)
                )
            );

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

            state.visualizer.enabled = true;
            state.visualizer.mode = match mode_name.as_str() {
                "oscilloscope" => crate::visualizer::VisualizerMode::Oscilloscope,
                "spectrum" => crate::visualizer::VisualizerMode::Spectrum,
                "analog_vu" => crate::visualizer::VisualizerMode::AnalogVu,
                _ => crate::visualizer::VisualizerMode::None,
            };

            state.visualizer.frame.left_points = left_points.unwrap_or_default();
            state.visualizer.frame.right_points = right_points.unwrap_or_default();
        }

        let output = match
            Command::new(&ctx.config.paths.songrec_bin)
                .args(["recognize", &ctx.config.audio.sample_wav, "--json"])
                .output()
        {
            Ok(output) => output,
            Err(e) => {
                log_error(&ctx, &format!("Failed to execute songrec: {e}"));
                thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !running.load(Ordering::SeqCst) {
            break;
        }

        log_debug(&ctx, &format!("SongRec exit status: {}", output.status));
        if !stderr.trim().is_empty() {
            log_debug(&ctx, "SongRec stderr:");
            log_debug(&ctx, stderr.trim());
        }

        if stdout.trim().is_empty() {
            log_info(&ctx, "No JSON returned.");
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        let json: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(e) => {
                log_error(&ctx, &format!("No match or bad JSON: {e}"));
                thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
                continue;
            }
        };

        let title = json["track"]["title"].as_str().unwrap_or("Unknown");
        let artist = json["track"]["subtitle"].as_str().unwrap_or("Unknown");
        let album = extract_album(&json);
        let track_number = extract_track_number(&json);
        let composer = extract_composer(&json);
        let released = extract_released(&json);
        let genre = extract_genre(&json);
        let label = extract_label(&json);
        let notes = extract_notes(&json);

        let current = format!("{artist} - {title}");

        let preview_url = pick_artwork_url(&json).unwrap_or_default();
        if preview_url.is_empty() {
            log_info(&ctx, &format!("No artwork URL for {current}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        if current == last_track && preview_url == last_artwork_url {
            log_info(&ctx, &format!("Same track and artwork: {current}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        log_blank(&ctx);
        log_info(&ctx, "========================================");
        log_info(&ctx, "NOW PLAYING");
        log_info(&ctx, &format!("Song Title:   {title}"));
        log_info(&ctx, &format!("Artist:       {artist}"));
        log_info(&ctx, &format!("Album:        {album}"));
        log_info(&ctx, &format!("Track:        {track_number}"));
        log_info(&ctx, &format!("Composer:     {composer}"));
        log_info(&ctx, &format!("Released:     {released}"));
        log_info(&ctx, &format!("Genre:        {genre}"));
        log_info(&ctx, &format!("Label:        {label}"));
        log_info(&ctx, &format!("Seed URL:     {preview_url}"));
        log_info(&ctx, &format!("Notes:        {notes}"));
        log_info(&ctx, "========================================");
        log_blank(&ctx);

        match download_best_artwork(&ctx, &json, &ctx.config.paths.artwork_file) {
            Ok(final_url) => {
                log_info(&ctx, &format!("Final URL:    {final_url}"));

                let artwork_changed = final_url != last_artwork_url;

                if artwork_changed {
                    let mut state = shared_state.lock().unwrap();
                    state.title = title.to_string();
                    state.artist = artist.to_string();
                    state.album = album;
                    state.track_number = track_number;
                    state.composer = composer;
                    state.released = released;
                    state.genre = genre;
                    state.label = label;
                    state.notes = notes;
                    state.artwork_path = ctx.config.paths.artwork_file.clone();
                    state.artwork_url = final_url.clone();
                    state.version = state.version.wrapping_add(1);
                    log_info(&ctx, "Updated UI state with new artwork.");
                } else {
                    log_info(&ctx, "Artwork unchanged, skipping UI state refresh.");
                }

                last_track = current;
                last_artwork_url = final_url;
            }
            Err(e) => {
                log_error(&ctx, &format!("Failed to download artwork: {e}"));
            }
        }

        if running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
        }
    }

    log_info(&ctx, "Recognition loop stopped.");
}
