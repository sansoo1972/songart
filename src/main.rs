mod audio;
mod config;
mod display;
mod logging;
mod recognition;
mod state;
mod visualizer;
mod visualizer_loop;

use crate::config::load_config;
use crate::display::run_display_loop;
use crate::logging::{ parse_log_level, reset_log_file, should_log, LogLevel };
use crate::recognition::run_recognition_loop;
use crate::state::{ AppContext, SongState };
use crate::visualizer_loop::run_visualizer_loop;

use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };
use std::thread;

fn main() {
    let config = load_config("config/songart.toml").expect("failed to load config/songart.toml");

    let ctx = Arc::new(AppContext {
        log_level: parse_log_level(&config.logging.level),
        config,
    });

    if ctx.config.logging.reset_on_start && should_log(&ctx, LogLevel::Debug) {
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

    let recognizer_running = Arc::clone(&running);
    let recognizer_state = Arc::clone(&shared_state);
    let recognizer_ctx = Arc::clone(&ctx);

    let recognizer = thread::spawn(move || {
        run_recognition_loop(recognizer_ctx, recognizer_running, recognizer_state);
    });

    let visualizer_running = Arc::clone(&running);
    let visualizer_state = Arc::clone(&shared_state);
    let visualizer_ctx = Arc::clone(&ctx);

    let visualizer = thread::spawn(move || {
        run_visualizer_loop(visualizer_ctx, visualizer_running, visualizer_state);
    });

    let display_result = run_display_loop(
        Arc::clone(&ctx),
        Arc::clone(&running),
        Arc::clone(&shared_state)
    );

    running.store(false, Ordering::SeqCst);
    let _ = recognizer.join();
    let _ = visualizer.join();

    if let Err(e) = display_result {
        logging::log_error(&ctx, &format!("Display loop error: {e}"));
    }

    logging::log_info(&ctx, "songart stopped.");
}