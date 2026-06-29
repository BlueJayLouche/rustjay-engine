//! QPlayer Protocols — OSC, MSC, and MIDI.

pub mod midi;
pub mod osc;
pub mod msc;

/// Lock a `Mutex` while tolerating poisoning, so a panicking protocol handler
/// can't kill the receive thread (or stop dispatch) on its next lock. Mirrors
/// `qplayer_core::LockExt` — duplicated to keep this crate dependency-free.
pub(crate) trait LockExt<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                self.clear_poison();
                poisoned.into_inner()
            }
        }
    }
}
