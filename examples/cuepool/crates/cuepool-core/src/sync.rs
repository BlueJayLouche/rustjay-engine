//! Poison-tolerant mutex locking.
//!
//! A `std::sync::Mutex` becomes *poisoned* if a thread panics while holding it,
//! after which every `.lock().unwrap()` panics in turn — one thread's crash
//! cascades into a dead application. For an unattended installation that means a
//! black gallery. [`LockExt::lock_unpoisoned`] recovers the guard instead.

use std::sync::{Mutex, MutexGuard};

pub trait LockExt<T> {
    /// Acquire the lock, recovering the guard even if a previous holder panicked.
    ///
    /// The poison flag is cleared on recovery so later plain `.lock()` callers
    /// succeed again (otherwise they would keep hitting the poison forever).
    ///
    /// Trade-off: data written by the panicking thread may be partial. For show
    /// control, continuing with possibly-stale state beats crashing the whole app.
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                self.clear_poison();
                poisoned.into_inner()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn recovers_and_clears_poison() {
        let m = Arc::new(Mutex::new(5));
        let m2 = Arc::clone(&m);
        // Poison the mutex by panicking while it is held.
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison it");
        })
        .join();
        assert!(m.is_poisoned());

        // Recovers the value despite the poison...
        assert_eq!(*m.lock_unpoisoned(), 5);
        // ...and clears the flag so a plain lock() works again (no cascade).
        assert!(!m.is_poisoned());
        assert_eq!(*m.lock().unwrap(), 5);
    }
}
