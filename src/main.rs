//! songart main runtime loop.
//!
//! This binary listens to ambient audio, identifies the current song using
//! SongRec, downloads the best available artwork, and displays it fullscreen
//! on the Pi framebuffer via `fbi`.

use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::Command;
use std::{thread, time::Duration};

/// When true, extra debug information is printed and appended to a logfile.
const VERBOSE: bool = true;

/// Local logfile path used when verbose logging is enabled.
const LOG_FILE: &str = "/home/admin/projects/songart/songart.log";

/// Audio sample file recorded before each SongRec recognition pass.
const SAMPLE_WAV: &str = "sample.wav";

/// Local artwork file displayed by `fbi`.
const CURRENT_ARTWORK: &str = "/home/admin/projects/songart/current.jpg";

/// Full path to the SongRec binary on the Pi.
const SONGREC_BIN: &str = "/home/admin/projects/vendor/songrec/target/release/songrec";

/// Name of the remapped mono audio source created for the PS3 Eye.
const AUDIO_DEVICE: &str = "ps3eye_mono";

/// Recording duration for each recognition attempt.
const RECORD_SECONDS: &str = "10s";

/// Sleep time between loop iterations.
const LOOP_DELAY_SECS: u64 = 2;

/// Appends a message to the logfile when verbose logging is enabled.
///
/// This is intentionally best-effort. Logging failures should not stop
/// the recognition/display loop.
fn log_debug(message: &str) {
    if !VERBOSE {
        return;
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(LOG_FILE) {
        let _ = writeln!(file, "{}", message);
    }
}

/// Prints a message to stdout and optionally mirrors it into the logfile.
fn log_line(message: &str) {
    println!("{message}");
    log_debug(message);
}

/// Writes a blank line to stdout and the logfile when verbose logging is enabled.
fn log_blank() {
    println!();
    log_debug("");
}

/// Pulls a metadata value out of the nested SongRec/Shazam JSON sections by title.
///
/// Example titles seen in metadata include:
/// - Album
/// - Label
/// - Released
/// - Composer
/// - Track
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

/// Extracts album title from SongRec metadata.
fn extract_album(json: &Value) -> String {
    metadata_value(json, "Album").unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts label/publisher from SongRec metadata.
fn extract_label(json: &Value) -> String {
    metadata_value(json, "Label").unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts released year/date from SongRec metadata.
fn extract_released(json: &Value) -> String {
    metadata_value(json, "Released").unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts composer/writer information from SongRec metadata.
///
/// Different tracks expose this under different field names.
fn extract_composer(json: &Value) -> String {
    metadata_value(json, "Composer")
        .or_else(|| metadata_value(json, "Writers"))
        .or_else(|| metadata_value(json, "Writer"))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts track number from SongRec metadata if present.
///
/// Note: total track count is often not exposed in SongRec JSON, so this may
/// only return a simple track number or "Unknown".
fn extract_track_number(json: &Value) -> String {
    metadata_value(json, "Track")
        .or_else(|| metadata_value(json, "Track Number"))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts the primary genre from the top-level track object.
fn extract_genre(json: &Value) -> String {
    json["track"]["genres"]["primary"]
        .as_str()
        .unwrap_or("Unknown")
        .to_string()
}

/// Extracts the ISRC if present.
fn extract_isrc(json: &Value) -> String {
    json["track"]["isrc"]
        .as_str()
        .unwrap_or("Unknown")
        .to_string()
}

/// Builds a short notes/trivia line from any useful metadata that is available.
///
/// This is intentionally lightweight and only uses data already present in the
/// SongRec response.
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
///
/// For Apple-hosted `mzstatic.com` images, this function tries to upgrade
/// lower-resolution `400x400` URLs to larger variants first, then falls back
/// to the original URL last.
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

    // Keep the original URL as the final fallback.
    out.push(url.to_string());
    out.dedup();
    out
}

/// Picks the first available seed artwork URL from the JSON response.
///
/// This does not download anything; it only decides what base image URL should
/// be used to generate candidate artwork URLs.
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

/// Downloads the best available artwork candidate and writes it atomically.
///
/// The function:
/// 1. gathers artwork URLs from the JSON
/// 2. expands Apple-hosted images into larger candidate sizes
/// 3. tries each candidate in order
/// 4. saves to `output_path.tmp` first
/// 5. renames it into place on success
///
/// Returns the winning artwork URL if successful.
fn download_best_artwork(json: &Value, output_path: &str) -> Result<String, String> {
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

    let client = reqwest::blocking::Client::builder()
        .user_agent("songart/0.1")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let mut candidates = Vec::new();
    for url in base_urls {
        candidates.extend(artwork_candidates(&url));
    }

    candidates.dedup();

    for candidate in candidates {
        log_line(&format!("Trying artwork: {candidate}"));

        let resp = match client.get(&candidate).send() {
            Ok(r) => r,
            Err(e) => {
                log_line(&format!("Download failed: {e}"));
                continue;
            }
        };

        if !resp.status().is_success() {
            log_line(&format!("HTTP status {} for {}", resp.status(), candidate));
            continue;
        }

        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => {
                log_line(&format!("Failed reading bytes: {e}"));
                continue;
            }
        };

        // Reject obviously tiny placeholder or bad-image responses.
        if bytes.len() < 10_000 {
            log_line(&format!(
                "Rejected tiny image ({} bytes): {}",
                bytes.len(),
                candidate
            ));
            continue;
        }

        let tmp_path = format!("{output_path}.tmp");

        fs::write(&tmp_path, &bytes)
            .map_err(|e| format!("Failed to save temp artwork to {}: {e}", tmp_path))?;

        fs::rename(&tmp_path, output_path)
            .map_err(|e| format!("Failed to rename temp artwork to {}: {e}", output_path))?;

        return Ok(candidate);
    }

    Err("No usable artwork URL succeeded".to_string())
}

fn main() {
    // Track the last displayed song and last displayed artwork URL so we can
    // suppress redundant downloads and framebuffer refreshes.
    let mut last_track = String::new();
    let mut last_artwork_url = String::new();

    if VERBOSE {
        log_line("Verbose logging enabled.");
        log_line(&format!("Log file: {LOG_FILE}"));
    }

    loop {
        // -----------------------------------------------------------------
        // 1. Record a short audio sample from the configured microphone source
        // -----------------------------------------------------------------
        log_line("Listening...");

        let record_status = Command::new("timeout")
            .args([
                RECORD_SECONDS,
                "parecord",
                "--device",
                AUDIO_DEVICE,
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "s16le",
                SAMPLE_WAV,
            ])
            .status();

        match record_status {
            Ok(status) => {
                if VERBOSE {
                    log_line(&format!("Record command exit status: {status}"));
                }
            }
            Err(e) => {
                log_line(&format!("Failed to record sample audio: {e}"));
                thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
                continue;
            }
        }

        // -----------------------------------------------------------------
        // 2. Run SongRec on the recorded WAV file and capture JSON output
        // -----------------------------------------------------------------
        let output = match Command::new(SONGREC_BIN)
            .args(["recognize", SAMPLE_WAV, "--json"])
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                log_line(&format!("Failed to execute songrec: {e}"));
                thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if VERBOSE {
            log_line(&format!("SongRec exit status: {}", output.status));
            if !stderr.trim().is_empty() {
                log_line("SongRec stderr:");
                log_line(stderr.trim());
            }
        }

        if stdout.trim().is_empty() {
            log_line("No JSON returned.");
            thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
            continue;
        }

        // -----------------------------------------------------------------
        // 3. Parse the SongRec JSON payload
        // -----------------------------------------------------------------
        let json: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(e) => {
                log_line(&format!("No match or bad JSON: {e}"));
                thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
                continue;
            }
        };

        // -----------------------------------------------------------------
        // 4. Extract human-readable metadata for logging and display logic
        // -----------------------------------------------------------------
        let title = json["track"]["title"].as_str().unwrap_or("Unknown");
        let artist = json["track"]["subtitle"].as_str().unwrap_or("Unknown");
        let album = extract_album(&json);
        let track_number = extract_track_number(&json);
        let composer = extract_composer(&json);
        let released = extract_released(&json);
        let genre = extract_genre(&json);
        let label = extract_label(&json);
        let notes = extract_notes(&json);

        let current = format!("{} - {}", artist, title);

        // -----------------------------------------------------------------
        // 5. Pick an artwork seed URL before doing any downloads
        // -----------------------------------------------------------------
        let preview_url = pick_artwork_url(&json).unwrap_or_default();
        if preview_url.is_empty() {
            log_line(&format!("No artwork URL for {current}"));
            thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
            continue;
        }

        // -----------------------------------------------------------------
        // 6. Skip work if both track and seed artwork are unchanged
        // -----------------------------------------------------------------
        if current == last_track && preview_url == last_artwork_url {
            log_line(&format!("Same track and artwork: {current}"));
            thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
            continue;
        }

        // -----------------------------------------------------------------
        // 7. Print the debug "NOW PLAYING" block only when track/artwork changes
        // -----------------------------------------------------------------
        log_blank();
        log_line("========================================");
        log_line("NOW PLAYING");
        log_line(&format!("Song Title:   {title}"));
        log_line(&format!("Artist:       {artist}"));
        log_line(&format!("Album:        {album}"));
        log_line(&format!("Track:        {track_number}"));
        log_line(&format!("Composer:     {composer}"));
        log_line(&format!("Released:     {released}"));
        log_line(&format!("Genre:        {genre}"));
        log_line(&format!("Label:        {label}"));
        log_line(&format!("Seed URL:     {preview_url}"));
        log_line(&format!("Notes:        {notes}"));
        log_line("========================================");
        log_blank();

        // -----------------------------------------------------------------
        // 8. Download the best available artwork and update the display if needed
        // -----------------------------------------------------------------
        match download_best_artwork(&json, CURRENT_ARTWORK) {
            Ok(final_url) => {
                log_line(&format!("Final URL:    {final_url}"));

                let artwork_changed = final_url != last_artwork_url;

                if artwork_changed {
                    log_line("Refreshing display...");

                    let pkill_status = Command::new("sudo").args(["pkill", "fbi"]).status();
                    if VERBOSE {
                        match pkill_status {
                            Ok(status) => log_line(&format!("pkill fbi exit status: {status}")),
                            Err(e) => log_line(&format!("pkill fbi failed: {e}")),
                        }
                    }

                    let fbi_status = Command::new("sudo")
                        .args([
                            "fbi",
                            "-T",
                            "1",
                            "-d",
                            "/dev/fb0",
                            "--noverbose",
                            "-a",
                            CURRENT_ARTWORK,
                        ])
                        .status();

                    if VERBOSE {
                        match fbi_status {
                            Ok(status) => log_line(&format!("fbi exit status: {status}")),
                            Err(e) => log_line(&format!("fbi failed: {e}")),
                        }
                    }
                } else {
                    log_line("Artwork unchanged, skipping display refresh.");
                }

                // Update the last seen state only after successful artwork handling.
                last_track = current;
                last_artwork_url = final_url;
            }
            Err(e) => {
                log_line(&format!("Failed to download artwork: {e}"));
            }
        }

        // -----------------------------------------------------------------
        // 9. Pause briefly before the next recognition cycle
        // -----------------------------------------------------------------
        thread::sleep(Duration::from_secs(LOOP_DELAY_SECS));
    }
}
