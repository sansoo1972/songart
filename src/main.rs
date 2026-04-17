//! songart main runtime.
//!
//! This binary:
//! 1. loads runtime configuration from TOML
//! 2. records a short audio sample
//! 3. identifies the song with SongRec
//! 4. downloads high-resolution artwork
//! 5. renders a fullscreen split layout:
//!    - artwork on top
//!    - metadata panel underneath

mod config;

use crate::config::{AppConfig, FontTheme, load_config};
use sdl2::event::Event;
use sdl2::image::{InitFlag, LoadTexture};
use sdl2::keyboard::Keycode;
use sdl2::pixels::Color;
use sdl2::rect::Rect;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

/// Logging severity used to control how noisy the app is.
///
/// Lower values are more important. Higher values are noisier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LogLevel {
    Error = 1,
    Info = 2,
    Debug = 3,
}

/// Shared runtime context.
///
/// This makes configuration and derived logging state easy to pass around.
#[derive(Clone)]
struct AppContext {
    config: AppConfig,
    log_level: LogLevel,
}

/// Shared UI state consumed by the SDL renderer.
///
/// The recognition thread updates this when a new track/artwork is found.
/// The display loop reads it to redraw the screen.
#[derive(Clone, Debug)]
struct SongState {
    title: String,
    artist: String,
    album: String,
    track_number: String,
    composer: String,
    released: String,
    genre: String,
    label: String,
    notes: String,
    artwork_path: String,
    artwork_url: String,
    version: u64,
}

impl Default for SongState {
    /// Provides friendly placeholder text so the renderer does not attempt
    /// to draw empty strings before the first song is recognized.
    fn default() -> Self {
        Self {
            title: "Waiting for music...".to_string(),
            artist: "No track identified yet".to_string(),
            album: "Album unknown".to_string(),
            track_number: "Unknown".to_string(),
            composer: "Unknown".to_string(),
            released: "Unknown".to_string(),
            genre: "Unknown".to_string(),
            label: "Unknown".to_string(),
            notes: "Listening for audio input".to_string(),
            artwork_path: String::new(),
            artwork_url: String::new(),
            version: 0,
        }
    }
}

/// Converts a configured log level string into the enum used by the app.
fn parse_log_level(level: &str) -> LogLevel {
    match level.to_ascii_lowercase().as_str() {
        "error" => LogLevel::Error,
        "info" => LogLevel::Info,
        "debug" => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

/// Returns `true` when a message at `level` should be logged.
fn should_log(ctx: &AppContext, level: LogLevel) -> bool {
    level <= ctx.log_level
}

/// Builds a simple timestamp string.
///
/// This keeps dependencies minimal. It uses epoch seconds.
fn timestamp_string() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => dur.as_secs().to_string(),
        Err(_) => "0".to_string(),
    }
}

/// Truncates the log file on startup in debug mode so test runs start fresh.
fn reset_log_file(ctx: &AppContext) {
    let _ = fs::write(&ctx.config.logging.file, "");
}

/// Writes a log message to stdout and to the configured logfile when enabled.
///
/// Messages are prefixed with a timestamp and level.
fn log_message(ctx: &AppContext, level: LogLevel, message: &str) {
    if !should_log(ctx, level) {
        return;
    }

    let line = format!("[{}] [{:?}] {}", timestamp_string(), level, message);
    println!("{line}");

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ctx.config.logging.file)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Logs an error-level message.
fn log_error(ctx: &AppContext, message: &str) {
    log_message(ctx, LogLevel::Error, message);
}

/// Logs an info-level message.
fn log_info(ctx: &AppContext, message: &str) {
    log_message(ctx, LogLevel::Info, message);
}

/// Logs a debug-level message.
fn log_debug(ctx: &AppContext, message: &str) {
    log_message(ctx, LogLevel::Debug, message);
}

/// Writes a blank line to stdout and the logfile.
///
/// Useful for visual separation in logs.
fn log_blank(ctx: &AppContext) {
    if !should_log(ctx, LogLevel::Info) {
        return;
    }

    println!();

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ctx.config.logging.file)
    {
        let _ = writeln!(file);
    }
}

/// Pulls a metadata value out of the nested SongRec/Shazam JSON sections by title.
///
/// Example metadata titles include:
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

/// Extracts album title.
fn extract_album(json: &Value) -> String {
    metadata_value(json, "Album").unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts label/publisher.
fn extract_label(json: &Value) -> String {
    metadata_value(json, "Label").unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts release year/date.
fn extract_released(json: &Value) -> String {
    metadata_value(json, "Released").unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts composer or writer information.
fn extract_composer(json: &Value) -> String {
    metadata_value(json, "Composer")
        .or_else(|| metadata_value(json, "Writers"))
        .or_else(|| metadata_value(json, "Writer"))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts track number if present.
fn extract_track_number(json: &Value) -> String {
    metadata_value(json, "Track")
        .or_else(|| metadata_value(json, "Track Number"))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts primary genre.
fn extract_genre(json: &Value) -> String {
    json["track"]["genres"]["primary"]
        .as_str()
        .unwrap_or("Unknown")
        .to_string()
}

/// Extracts ISRC.
fn extract_isrc(json: &Value) -> String {
    json["track"]["isrc"]
        .as_str()
        .unwrap_or("Unknown")
        .to_string()
}

/// Builds a short notes/trivia line from available metadata.
///
/// This intentionally uses only data already present in SongRec JSON.
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
/// For Apple-hosted artwork, larger variants are tried first.
/// The original URL is retained as the final fallback.
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

    // Keep the original URL as a fallback.
    out.push(url.to_string());
    out.dedup();
    out
}

/// Picks the first available seed artwork URL from the JSON response.
///
/// This does not download anything. It just selects the base URL set.
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
///
/// The file is written to `output_path.tmp` first, then renamed into place.
fn download_best_artwork(
    ctx: &AppContext,
    json: &Value,
    output_path: &str,
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
        log_debug(ctx, &format!("Trying artwork: {candidate}"));

        let resp = match client.get(&candidate).send() {
            Ok(r) => r,
            Err(e) => {
                log_debug(ctx, &format!("Download failed: {e}"));
                continue;
            }
        };

        if !resp.status().is_success() {
            log_debug(
                ctx,
                &format!("HTTP status {} for {}", resp.status(), candidate),
            );
            continue;
        }

        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => {
                log_debug(ctx, &format!("Failed reading bytes: {e}"));
                continue;
            }
        };

        // Reject obviously tiny placeholder responses.
        if bytes.len() < 10_000 {
            log_debug(
                ctx,
                &format!("Rejected tiny image ({} bytes): {}", bytes.len(), candidate),
            );
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

/// Truncates long strings so they fit better in the metadata panel.
fn ellipsize(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }

    let trimmed: String = chars
        .into_iter()
        .take(max_chars.saturating_sub(1))
        .collect();
    format!("{trimmed}…")
}

/// Renders a single line of text.
///
/// Empty strings are converted to a single space so SDL_ttf does not error
/// with "Text has zero width".
fn draw_text_line(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture_creator: &sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    font: &sdl2::ttf::Font,
    text: &str,
    color: Color,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let safe_text = if text.trim().is_empty() { " " } else { text };

    let surface = font
        .render(safe_text)
        .blended(color)
        .map_err(|e| e.to_string())?;

    let texture = texture_creator
        .create_texture_from_surface(&surface)
        .map_err(|e| e.to_string())?;

    let target = Rect::new(x, y, surface.width(), surface.height());
    canvas.copy(&texture, None, target)?;
    Ok(())
}

/// Background recognition loop.
///
/// This thread:
/// - records audio
/// - calls SongRec
/// - parses metadata
/// - downloads artwork
/// - updates shared state for the renderer
fn run_recognition_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
) {
    let mut last_track = String::new();
    let mut last_artwork_url = String::new();

    log_info(&ctx, &format!("Log file: {}", ctx.config.logging.file));
    log_info(&ctx, "Recognition loop started.");

    while running.load(Ordering::SeqCst) {
        // 1. Record a short audio sample.
        log_info(&ctx, "Listening...");

        let record_status = Command::new("timeout")
            .args([
                format!("{}s", ctx.config.audio.record_seconds).as_str(),
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

        // 2. Run SongRec on the captured WAV file.
        let output = match Command::new(&ctx.config.paths.songrec_bin)
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

        // 3. Parse the SongRec JSON payload.
        let json: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(e) => {
                log_error(&ctx, &format!("No match or bad JSON: {e}"));
                thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
                continue;
            }
        };

        // 4. Extract metadata for logging and display.
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

        // 5. Pick an artwork seed URL before downloading anything.
        let preview_url = pick_artwork_url(&json).unwrap_or_default();
        if preview_url.is_empty() {
            log_info(&ctx, &format!("No artwork URL for {current}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        // 6. Skip redundant work if track and artwork seed are unchanged.
        if current == last_track && preview_url == last_artwork_url {
            log_info(&ctx, &format!("Same track and artwork: {current}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        // 7. Print a structured now-playing block when the song changes.
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

        // 8. Download artwork and update shared UI state.
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

/// Returns the effective window size after applying the configured orientation.
///
/// If portrait is requested but width > height, swap them.
/// If landscape is requested but height > width, swap them.
fn configured_window_size(ctx: &AppContext) -> (u32, u32) {
    let w = ctx.config.display.width;
    let h = ctx.config.display.height;

    match ctx.config.display.orientation.to_ascii_lowercase().as_str() {
        "portrait" if w > h => (h, w),
        "landscape" if h > w => (h, w),
        _ => (w, h),
    }
}

/// Returns the effective top-panel ratio for the current orientation.
///
/// Portrait mode gets a slightly smaller ratio than landscape so the
/// metadata panel still has room, but the overall taller screen still
/// allows a larger square cover.
fn effective_top_panel_ratio(ctx: &AppContext) -> f32 {
    match ctx.config.display.orientation.to_ascii_lowercase().as_str() {
        "portrait" => 0.68,
        "landscape" => ctx.config.display.top_panel_ratio,
        _ => ctx.config.display.top_panel_ratio,
    }
}

/// Returns true when the display is configured for portrait orientation.
fn is_portrait(ctx: &AppContext) -> bool {
    ctx.config
        .display
        .orientation
        .eq_ignore_ascii_case("portrait")
}

/// Resolves the configured font theme into title/body font paths.
///
/// Falls back to system DejaVu Sans if the configured theme is missing.
fn selected_fonts<'a>(ctx: &'a AppContext) -> (&'a str, &'a str) {
    let theme_name = ctx.config.fonts.theme.to_ascii_lowercase();

    if let Some(theme) = ctx.config.font_themes.get(&theme_name) {
        (&theme.title, &theme.body)
    } else {
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        )
    }
}

/// SDL display loop.
///
/// This runs on the main thread and owns the full screen.
/// It redraws continuously and reloads the artwork texture only when the
/// shared state version changes.
fn run_display_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _image_ctx = sdl2::image::init(InitFlag::JPG | InitFlag::PNG)?;
    let ttf_ctx = sdl2::ttf::init().map_err(|e| e.to_string())?;

    let (window_w, window_h) = configured_window_size(&ctx);

    let mut window_builder = video.window(&ctx.config.display.window_title, window_w, window_h);
    window_builder.position_centered();

    let window = if ctx.config.display.fullscreen {
        window_builder
            .fullscreen_desktop()
            .build()
            .map_err(|e| e.to_string())?
    } else {
        window_builder.build().map_err(|e| e.to_string())?
    };

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .map_err(|e| e.to_string())?;

    let texture_creator = canvas.texture_creator(); 

    // Resolve the configured theme into concrete title/body font files.
    let (title_font_path, body_font_path) = selected_fonts(&ctx);

    let title_font = ttf_ctx
        .load_font(title_font_path, ctx.config.fonts.title_size)
        .map_err(|e| format!("Failed to load title font from {}: {e}", title_font_path))?;

    let body_font = ttf_ctx
        .load_font(body_font_path, ctx.config.fonts.body_size)
        .map_err(|e| format!("Failed to load body font from {}: {e}", body_font_path))?;

    let mut event_pump = sdl.event_pump()?;
    let mut loaded_version: u64 = u64::MAX;
    let mut artwork_texture: Option<sdl2::render::Texture<'_>> = None;

    log_info(&ctx, "Display loop started.");

    while running.load(Ordering::SeqCst) {
        // Handle keyboard/window events.
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    running.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }

        // Snapshot shared state once per frame.
        let state = {
            let state_guard = shared_state.lock().unwrap();
            state_guard.clone()
        };

        // Reload artwork texture only when a new version arrives.
        if state.version != loaded_version {
            if !state.artwork_path.is_empty() && Path::new(&state.artwork_path).exists() {
                match texture_creator.load_texture(&state.artwork_path) {
                    Ok(texture) => {
                        artwork_texture = Some(texture);
                        loaded_version = state.version;
                        log_debug(
                            &ctx,
                            &format!(
                                "Renderer loaded artwork version {} from {}",
                                loaded_version, state.artwork_path
                            ),
                        );
                    }
                    Err(e) => {
                        log_error(&ctx, &format!("Renderer failed to load artwork: {e}"));
                    }
                }
            }
        }

        // Compute layout regions.
        //
        // In portrait mode we use a slightly different panel split so the artwork
        // can stay large while still leaving space for metadata below.
        let (win_w, win_h) = canvas.output_size().map_err(|e| e.to_string())?;
        let top_h = ((win_h as f32) * effective_top_panel_ratio(&ctx)) as u32;
        let bottom_h = win_h - top_h;
        let portrait = is_portrait(&ctx);

        // Clear background.
        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();

        // Draw artwork in the top region.
        //
        // For portrait mode, use slightly smaller side padding so the square cover
        // can grow as large as the narrower screen allows.
        if let Some(texture) = artwork_texture.as_ref() {
            let query = texture.query();
            let art_w = query.width as f32;
            let art_h = query.height as f32;

            let padding = if portrait { 16.0 } else { 24.0 };
            let max_w = win_w as f32 - (padding * 2.0);
            let max_h = top_h as f32 - (padding * 2.0);

            let scale = f32::min(max_w / art_w, max_h / art_h);
            let draw_w = (art_w * scale) as u32;
            let draw_h = (art_h * scale) as u32;

            let x = ((win_w - draw_w) / 2) as i32;
            let y = ((top_h - draw_h) / 2) as i32;

            canvas.copy(texture, None, Rect::new(x, y, draw_w, draw_h))?;
        }

        // Draw bottom metadata panel.
        canvas.set_draw_color(Color::RGB(18, 18, 18));
        canvas.fill_rect(Rect::new(0, top_h as i32, win_w, bottom_h))?;

        // Position the metadata panel slightly differently in portrait mode so the
        // text stays visually balanced below the larger artwork block.
        let panel_x = if portrait { 28 } else { 40 };
        let mut panel_y = if portrait {
            top_h as i32 + 22
        } else {
            top_h as i32 + 28
        };

        let title_line = ellipsize(
            if state.title.trim().is_empty() {
                "Waiting for music..."
            } else {
                &state.title
            },
            48,
        );

        let artist_line = ellipsize(
            if state.artist.trim().is_empty() {
                "No track identified yet"
            } else {
                &state.artist
            },
            56,
        );

        let mut third_line = state.album.clone();
        if !state.released.is_empty() && state.released != "Unknown" {
            if third_line.is_empty() || third_line == "Unknown" {
                third_line = state.released.clone();
            } else {
                third_line = format!("{} • {}", state.album, state.released);
            }
        }

        let third_line = if third_line.trim().is_empty() {
            "Album unknown".to_string()
        } else {
            ellipsize(&third_line, 56)
        };

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &title_font,
            &title_line,
            Color::RGB(255, 255, 255),
            panel_x,
            panel_y,
        )?;
        panel_y += if portrait { 42 } else { 46 };

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &body_font,
            &artist_line,
            Color::RGB(210, 210, 210),
            panel_x,
            panel_y,
        )?;
        panel_y += if portrait { 30 } else { 34 };

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &body_font,
            &third_line,
            Color::RGB(180, 180, 180),
            panel_x,
            panel_y,
        )?;
        panel_y += if portrait { 34 } else { 40 };

        let detail_line = format!(
            "Genre: {}    Composer: {}",
            ellipsize(&state.genre, 20),
            ellipsize(&state.composer, 28)
        );

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &body_font,
            &detail_line,
            Color::RGB(140, 140, 140),
            panel_x,
            panel_y,
        )?;

        // Present the completed frame.
        canvas.present();

        thread::sleep(Duration::from_millis(ctx.config.display.frame_delay_ms));
    }

    log_info(&ctx, "Display loop stopped.");
    Ok(())
}

/// Program entry point.
///
/// Sets up Ctrl+C handling, loads config, starts the recognizer thread,
/// and runs the SDL display loop on the main thread.
fn main() {
    // Load runtime configuration first.
    let config = load_config("config/songart.toml").expect("failed to load config/songart.toml");

    let ctx = Arc::new(AppContext {
        log_level: parse_log_level(&config.logging.level),
        config,
    });

    // Reset the log file when configured to do so.
    if ctx.config.logging.reset_on_start && should_log(&ctx, LogLevel::Debug) {
        reset_log_file(&ctx);
    }

    // Shared shutdown flag used by both threads.
    let running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&running);

    ctrlc::set_handler(move || {
        running_flag.store(false, Ordering::SeqCst);
    })
    .expect("failed to set Ctrl-C handler");

    // Shared UI state passed between recognizer and renderer.
    let shared_state = Arc::new(Mutex::new(SongState::default()));

    // Spawn the background recognition thread.
    let recognizer_running = Arc::clone(&running);
    let recognizer_state = Arc::clone(&shared_state);
    let recognizer_ctx = Arc::clone(&ctx);

    let recognizer = thread::spawn(move || {
        run_recognition_loop(recognizer_ctx, recognizer_running, recognizer_state);
    });

    // Keep the renderer on the main thread.
    let display_result = run_display_loop(
        Arc::clone(&ctx),
        Arc::clone(&running),
        Arc::clone(&shared_state),
    );

    // Ensure the background thread is asked to stop and joined cleanly.
    running.store(false, Ordering::SeqCst);
    let _ = recognizer.join();

    if let Err(e) = display_result {
        log_error(&ctx, &format!("Display loop error: {e}"));
    }

    log_info(&ctx, "songart stopped.");
}
