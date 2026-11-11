//! Process lifecycle: startup recovery and the guaranteed shutdown path.
//!
//! Two invariants live here:
//!
//! * **Nothing is left behind.** [`shutdown`] deactivates the session before the
//!   process exits, so the power scheme, timer resolution, priorities, affinity
//!   and registry values all return to what they were.
//! * **A crash is survivable.** If a previous run died while active, its
//!   snapshot file is still on disk; [`recover`] replays it on the next start.

use std::sync::atomic::{AtomicBool, Ordering};

use gm_core::session::Session;

/// Set once shutdown has begun, so a double click on Quit cannot run it twice.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Restores anything a previous, crashed run left applied.
pub fn recover(session: &mut Session) -> Vec<String> {
    session.recover()
}

/// Deactivates the session and returns what was restored.
///
/// Safe to call more than once; only the first call does the work.
pub fn shutdown(session: &mut Session, reason: &str) -> Vec<String> {
    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        return Vec::new();
    }
    let restored = session.disable(reason);
    session.config.save().ok();
    restored
}
