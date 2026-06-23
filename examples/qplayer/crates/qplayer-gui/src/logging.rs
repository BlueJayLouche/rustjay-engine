//! Global log capture for the in-app log window.
//!
//! A custom `log::Log` implementation forwards to `env_logger` (stderr) and
//! also pushes every message into a bounded in-memory ring buffer that the
//! GUI can display.

use log::{Level, Log, Metadata, Record};
use std::collections::VecDeque;
use std::sync::Mutex;

const MAX_ENTRIES: usize = 2000;

/// A single captured log line.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: Level,
    pub target: String,
    pub message: String,
    pub timestamp: String,
}

/// Global ring buffer of recent log entries.
static LOG_BUFFER: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());

/// Initialize the dual logger (stderr + in-app buffer).
///
/// Call once at startup. Replaces `env_logger::init()`.
pub fn init_logger() {
    let mut builder = env_logger::Builder::from_default_env();
    builder.format_timestamp_millis();
    let env_logger = builder.build();

    let max_level = env_logger.filter();
    let dual = DualLogger { env_logger };

    log::set_boxed_logger(Box::new(dual))
        .map(|()| log::set_max_level(max_level))
        .expect("Failed to set logger");
}

/// Read a snapshot of the current log buffer.
pub fn read_log_buffer() -> Vec<LogEntry> {
    match LOG_BUFFER.lock() {
        Ok(buf) => buf.iter().cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// Clear the log buffer.
pub fn clear_log_buffer() {
    if let Ok(mut buf) = LOG_BUFFER.lock() {
        buf.clear();
    }
}

struct DualLogger {
    env_logger: env_logger::Logger,
}

impl Log for DualLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.env_logger.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // Forward to stderr via env_logger
        self.env_logger.log(record);

        // Push to in-app ring buffer
        let entry = LogEntry {
            level: record.level(),
            target: record.target().to_string(),
            message: format!("{}", record.args()),
            timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
        };

        if let Ok(mut buf) = LOG_BUFFER.lock() {
            if buf.len() >= MAX_ENTRIES {
                buf.pop_front();
            }
            buf.push_back(entry);
        }
    }

    fn flush(&self) {
        self.env_logger.flush();
    }
}
