//! Client half of the single-instance check.
//!
//! A second launch of `gamemode.exe` must not start a competing optimiser.
//! Instead it connects to the running instance's control pipe, sends one command
//! and exits. If no server answers, the caller becomes the primary instance and
//! owns the pipe for its lifetime.

use gm_core::win::ipc::{self, IpcCommand};

/// What a second launch should do to the running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Flip the current state.
    Toggle,
    /// Turn optimisation on.
    Enable,
    /// Turn optimisation off.
    Disable,
    /// Bring up the control panel.
    Show,
    /// Stop the running instance completely.
    Quit,
    /// No action, just check whether an instance is running.
    Status,
}

impl Verb {
    /// Maps a [`Verb`] onto the core IPC command set.
    pub fn to_ipc(self) -> IpcCommand {
        match self {
            Verb::Toggle => IpcCommand::Toggle,
            Verb::Enable => IpcCommand::Enable,
            Verb::Disable => IpcCommand::Disable,
            Verb::Show => IpcCommand::Show,
            Verb::Quit => IpcCommand::Quit,
            Verb::Status => IpcCommand::Status,
        }
    }
}

/// Hands `verb` to a running instance.
///
/// Returns `true` only when a live instance accepted the command, in which case
/// the caller must exit without starting its own session.
pub fn send_to_running(verb: Verb) -> bool {
    matches!(ipc::send_command(verb.to_ipc()), Ok(true))
}

/// Parses the first recognised switch from the arguments (program name already removed).
pub fn verb_from_args(args: &[String]) -> Option<Verb> {
    for a in args {
        let t = a.trim_start_matches('-').trim_start_matches('/').to_ascii_lowercase();
        match t.as_str() {
            "on" | "enable" | "start" => return Some(Verb::Enable),
            "off" | "disable" | "stop" => return Some(Verb::Disable),
            "toggle" => return Some(Verb::Toggle),
            "show" | "open" | "panel" => return Some(Verb::Show),
            "quit" | "exit" => return Some(Verb::Quit),
            "status" => return Some(Verb::Status),
            _ => {}
        }
    }
    None
}
