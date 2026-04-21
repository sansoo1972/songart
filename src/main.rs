mod audio;
mod config;
mod display;
mod logging;
mod recognition;
mod state;
mod visualizer;

use crate::audio::{ create_shared_audio_buffer, run_audio_capture_loop };
use crate::config::load_config;
use crate::display::run_display_loop;
use crate::logging::{ parse_log_level, reset_log_file, should_log, LogLevel };
use crate::recognition::run_recognition_loop;
use crate::state::{ AppContext, SongState };

use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };
use std::thread;

/// Application entry point.
///
/// Thread layout:
/// - audio capture thread: continuously fills the rolling live audio buffer
/// - recognition thread: periodically snapshots buffered audio for SongRec
/// - display loop: renders metadata/artwork and live oscilloscope
fn main() {
    let config = load_config("config/songart.toml").expect("failed to load config/songart.toml");

    let ctx = Arc::new(AppContext {
        log_level: parse_log_level(&config.logging.level),
        config,
    });

    if ctx.config.logging.reset_on_start {
        reset_log_file(&ctx);
    }

    let running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&running);

    ctrlc
        ::set_handler(move || {
            running_flag.store(false, Ordering::SeqCst);
        })
        .expect("failed to set Ctrl-C handler");

    let shared_state = Arc::new(Mutex::new(SongState::default()));
    let shared_audio = create_shared_audio_buffer(&ctx);

    let audio_running = Arc::clone(&running);
    let audio_ctx = Arc::clone(&ctx);
    let audio_buffer = Arc::clone(&shared_audio);

    let audio_thread = thread::spawn(move || {
        run_audio_capture_loop(audio_ctx, audio_running, audio_buffer);
    });

    let recognizer_running = Arc::clone(&running);
    let recognizer_state = Arc::clone(&shared_state);
    let recognizer_audio = Arc::clone(&shared_audio);
    let recognizer_ctx = Arc::clone(&ctx);

    let recognizer = thread::spawn(move || {
        run_recognition_loop(
            recognizer_ctx,
            recognizer_running,
            recognizer_state,
            recognizer_audio
        );
    });

    let display_result = run_display_loop(
        Arc::clone(&ctx),
        Arc::clone(&running),
        Arc::clone(&shared_state),
        Arc::clone(&shared_audio)
    );

    running.store(false, Ordering::SeqCst);

    let _ = recognizer.join();
    let _ = audio_thread.join();

    if let Err(e) = display_result {
        crate::logging::log_error(&ctx, &format!("Display loop error: {e}"));
    }

    crate::logging::log_info(&ctx, "songart stopped.");
}