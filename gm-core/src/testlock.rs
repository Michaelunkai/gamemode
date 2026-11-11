//! Serialises tests that touch machine-global state.
//!
//! Several tests change the active power scheme, the system timer resolution,
//! sleep inhibition or the priority of the test process itself. Those are
//! process-wide and machine-wide effects, so running them in parallel makes them
//! interfere with each other. Every such test takes this lock first.

use std::sync::{Mutex, MutexGuard, OnceLock};

static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Acquire the exclusive guard for machine-global tests.
pub fn guard() -> MutexGuard<'static, ()> {
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
