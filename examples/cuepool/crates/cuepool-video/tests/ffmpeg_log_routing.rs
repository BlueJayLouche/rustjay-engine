//! End-to-end check that FFmpeg's own log lines reach `log` intact.
//!
//! Worth a real test rather than inspection: the callback receives a
//! `va_list`, which is platform-specific and will happily compile while
//! producing garbage or worse at runtime.

use std::sync::{Mutex, OnceLock};

static CAPTURED: OnceLock<Mutex<Vec<(log::Level, String)>>> = OnceLock::new();

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        CAPTURED
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .push((record.level(), format!("{}", record.args())));
    }
    fn flush(&self) {}
}

#[test]
fn ffmpeg_errors_arrive_as_formatted_debug_records() {
    CAPTURED.get_or_init(Default::default);
    static CAPTURE: Capture = Capture;
    log::set_logger(&CAPTURE).expect("test owns the logger");
    log::set_max_level(log::LevelFilter::Trace);
    cuepool_video::install_ffmpeg_logging();

    // A file that is definitely not media: FFmpeg logs its complaint through
    // the callback while failing to open it.
    let path = std::env::temp_dir().join("cuepool-ffmpeg-log-routing.bin");
    std::fs::write(&path, vec![0xABu8; 4096]).unwrap();
    let _ = ffmpeg_next::format::input(&path);
    let _ = std::fs::remove_file(&path);

    let captured = CAPTURED.get().unwrap().lock().unwrap().clone();
    let ffmpeg: Vec<_> = captured
        .iter()
        .filter(|(_, msg)| msg.starts_with("[ffmpeg] "))
        .collect();

    assert!(
        !ffmpeg.is_empty(),
        "no FFmpeg records reached log; captured: {captured:?}"
    );
    for (level, msg) in &ffmpeg {
        // Nothing FFmpeg says outranks debug — a cancelled read reports as
        // an error and must not read as a fault.
        assert!(
            *level >= log::Level::Debug,
            "FFmpeg record logged at {level}: {msg}"
        );
        // Formatted, not a raw format string with unexpanded specifiers.
        assert!(
            !msg.contains("%s") && !msg.contains("%d"),
            "unformatted: {msg}"
        );
        assert!(msg.len() > "[ffmpeg] ".len(), "empty record: {msg}");
    }
    eprintln!(
        "captured {} FFmpeg records, e.g. {:?}",
        ffmpeg.len(),
        ffmpeg[0]
    );
}
