//! Route FFmpeg's own logging into `log` instead of raw stderr.
//!
//! FFmpeg writes to stderr at `AV_LOG_INFO` by default, bypassing the app's
//! logging entirely — so nothing it says reaches `cuepool.log`, and everything
//! it says lands on the operator's terminal unfiltered.
//!
//! That matters most during cancellation. Re-seeking a video cancels the
//! in-flight decode through `AVIOInterruptCB`, which FFmpeg polls *inside*
//! blocking I/O: the aborted read comes back short and the demuxer reports it
//! as damage — `partial file` per packet, or a truncated `STSZ` atom when the
//! interrupt lands mid-header. Normal teardown therefore reads as catastrophic
//! media failure, on a file that is perfectly intact. Chasing timecode makes
//! this constant, because every hard sync cancels a decode.
//!
//! Demoting those to `debug` keeps them for diagnosis without them looking
//! like a fault. Real failures still surface: the decode paths report their
//! own errors through `VideoSource`, independent of this.

use ffmpeg_next::ffi;
use std::ffi::{c_char, c_int, c_void};
use std::sync::Once;

/// How `va_list` reaches a callback differs by ABI: Apple and Windows alias it
/// to a pointer, while System V lowers it to `*mut __va_list_tag` in function
/// position. `av_log_set_callback` and `av_log_format_line2` agree on the shape
/// on any one target, so alias it once and pass it straight through — this code
/// never inspects it.
#[cfg(all(unix, not(target_vendor = "apple")))]
type VaList = *mut ffi::__va_list_tag;
#[cfg(not(all(unix, not(target_vendor = "apple"))))]
type VaList = ffi::va_list;

/// FFmpeg log levels used here (`libavutil/log.h`).
const AV_LOG_WARNING: c_int = 24;
const AV_LOG_VERBOSE: c_int = 40;

/// Longest formatted FFmpeg line kept; the rest is dropped. FFmpeg's own
/// default callback uses the same 1024-byte line buffer.
const LINE_CAPACITY: usize = 1024;

/// Send FFmpeg's logging to `log`. Idempotent — later calls do nothing.
pub fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe {
        // Everything up to verbose is offered to the callback; the callback
        // decides what is worth a record. Trace stays off — it is per-packet
        // and enormous.
        ffi::av_log_set_level(AV_LOG_VERBOSE);
        ffi::av_log_set_callback(Some(ffmpeg_log_callback));
    });
}

unsafe extern "C" fn ffmpeg_log_callback(
    avcl: *mut c_void,
    level: c_int,
    fmt: *const c_char,
    args: VaList,
) {
    if fmt.is_null() {
        return;
    }
    // Map first, so a level we do not record costs no formatting.
    let target_level = match level {
        // Panic/fatal/error. Demuxer complaints about truncated data live
        // here and are routinely just a cancelled read, so they are debug:
        // a genuine failure is reported by the decode path itself.
        l if l <= AV_LOG_WARNING => log::Level::Debug,
        _ => log::Level::Trace,
    };
    if !log::log_enabled!(target_level) {
        return;
    }

    let mut line = [0 as c_char; LINE_CAPACITY];
    // print_prefix is an in/out flag FFmpeg uses to continue partial lines;
    // each record here is standalone, so it always starts a fresh prefix.
    let mut print_prefix: c_int = 1;
    let written = unsafe {
        ffi::av_log_format_line2(
            avcl,
            level,
            fmt,
            args,
            line.as_mut_ptr(),
            LINE_CAPACITY as c_int,
            &mut print_prefix,
        )
    };
    if written <= 0 {
        return;
    }
    // av_log_format_line2 returns the length it *would* have written, so clamp
    // to the buffer before reading it back.
    let len = (written as usize).min(LINE_CAPACITY - 1);
    let bytes = unsafe { std::slice::from_raw_parts(line.as_ptr().cast::<u8>(), len) };
    let text = String::from_utf8_lossy(bytes);
    log::log!(target_level, "[ffmpeg] {}", text.trim_end());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installing must be safe to call from every entry point that touches
    /// video, and must not re-register the callback.
    #[test]
    fn install_is_idempotent() {
        install();
        install();
        install();
    }

    /// The mapping is the whole policy: demuxer "damage" reports arrive at
    /// error level during ordinary cancellation, so nothing FFmpeg says is
    /// louder than debug.
    #[test]
    fn no_ffmpeg_level_is_recorded_above_debug() {
        for level in [0, 8, 16, AV_LOG_WARNING, 32, AV_LOG_VERBOSE, 56] {
            let mapped = if level <= AV_LOG_WARNING {
                log::Level::Debug
            } else {
                log::Level::Trace
            };
            assert!(
                mapped >= log::Level::Debug,
                "level {level} mapped louder than debug"
            );
        }
    }
}
