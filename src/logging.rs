use crate::state::AppContext;
use std::fs::{self, OpenOptions};
use std::io::Write;

/// Logging severity used to control how noisy the app is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error = 1,
    Info = 2,
    Debug = 3,
}

/// Converts a configured log level string into the enum used by the app.
pub fn parse_log_level(level: &str) -> LogLevel {
    match level.to_ascii_lowercase().as_str() {
        "error" => LogLevel::Error,
        "info" => LogLevel::Info,
        "debug" => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

/// Returns `true` when a message at `level` should be logged.
pub fn should_log(ctx: &AppContext, level: LogLevel) -> bool {
    level <= ctx.log_level
}

/// Builds a simple timestamp string using epoch seconds.
fn timestamp_string() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => dur.as_secs().to_string(),
        Err(_) => "0".to_string(),
    }
}

/// Truncates the log file on startup in debug mode so test runs start fresh.
pub fn reset_log_file(ctx: &AppContext) {
    let _ = fs::write(&ctx.config.logging.file, "");
}

/// Writes a log message to stdout and to the logfile when enabled.
pub fn log_message(ctx: &AppContext, level: LogLevel, message: &str) {
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

pub fn log_error(ctx: &AppContext, message: &str) {
    log_message(ctx, LogLevel::Error, message);
}

pub fn log_info(ctx: &AppContext, message: &str) {
    log_message(ctx, LogLevel::Info, message);
}

pub fn log_debug(ctx: &AppContext, message: &str) {
    log_message(ctx, LogLevel::Debug, message);
}

pub fn log_blank(ctx: &AppContext) {
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