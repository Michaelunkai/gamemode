//! Thin, safe wrappers over the Windows APIs GameMode needs.
//!
//! Every submodule owns one concern so the optimiser logic above can stay free
//! of raw handles and access masks.

pub mod elevation;
pub mod ipc;
pub mod power;
pub mod process;
pub mod services;
pub mod system;
