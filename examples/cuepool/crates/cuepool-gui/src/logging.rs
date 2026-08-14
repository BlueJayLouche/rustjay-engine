//! Global log capture for the in-app log window.
//!
//! A custom `log::Log` implementation forwards to `env_logger` (stderr) and
//! also pushes every message into a bounded in-memory ring buffer that the
//! GUI can display.

use log::{Level, Log, Metadata, Record};
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_ENTRIES: usize = 2000;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;

/// A single captured log line.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub cursor: u64,
    pub level: Level,
    pub target: String,
    pub message: String,
    pub timestamp: String,
    pub recorded_at: String,
}

/// Global ring buffer of recent log entries.
static LOG_BUFFER: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static NEXT_CURSOR: AtomicU64 = AtomicU64::new(1);

/// Initialize the dual logger (stderr + in-app buffer).
///
/// Call once at startup. Replaces `env_logger::init()`.
pub fn init_logger() {
    let mut builder = env_logger::Builder::from_default_env();
    if std::env::var_os("RUST_LOG").is_none() {
        builder.filter_level(log::LevelFilter::Info);
    }
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

        // Allocate the cursor under the same lock as insertion so cursor order
        // is also buffer order for polling clients.
        if let Ok(mut buf) = LOG_BUFFER.lock() {
            let entry = LogEntry {
                cursor: NEXT_CURSOR.fetch_add(1, Ordering::Relaxed),
                level: record.level(),
                target: record.target().to_string(),
                message: bounded_message(*record.args()),
                timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
                recorded_at: chrono::Utc::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            };
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

fn bounded_message(arguments: std::fmt::Arguments<'_>) -> String {
    struct BoundedWriter {
        message: String,
        truncated: bool,
    }

    impl Write for BoundedWriter {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            let remaining = MAX_MESSAGE_BYTES.saturating_sub(self.message.len());
            if value.len() <= remaining {
                self.message.push_str(value);
            } else {
                let mut end = remaining;
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                self.message.push_str(&value[..end]);
                self.truncated = true;
            }
            Ok(())
        }
    }

    let mut writer = BoundedWriter {
        message: String::new(),
        truncated: false,
    };
    let _ = std::fmt::write(&mut writer, arguments);
    if writer.truncated {
        writer.message.push_str("… [truncated]");
    }
    writer.message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_log_messages_are_bounded_without_splitting_utf8() {
        let text = "🦀".repeat(MAX_MESSAGE_BYTES);
        let message = bounded_message(format_args!("{text}"));

        assert!(message.is_char_boundary(message.len()));
        assert!(message.starts_with('🦀'));
        assert!(message.ends_with("… [truncated]"));
        assert!(message.len() <= MAX_MESSAGE_BYTES + "… [truncated]".len());
    }
}
