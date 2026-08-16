//! CuePool Protocols — OSC, MSC, and MIDI.

pub mod ltc;
pub mod midi;
pub mod msc;
pub mod osc;
pub mod timecode;

/// Lock a `Mutex` while tolerating poisoning, so a panicking protocol handler
/// can't kill the receive thread (or stop dispatch) on its next lock. Mirrors
/// `cuepool_core::LockExt` — duplicated to keep this crate dependency-free.
pub(crate) trait LockExt<T> {
    #[track_caller]
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::error!(
                    "Recovered a poisoned mutex at {}: a protocol handler panicked while holding it, so this state may be partially written",
                    std::panic::Location::caller()
                );
                self.clear_poison();
                poisoned.into_inner()
            }
        }
    }
}
