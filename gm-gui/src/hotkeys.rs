//! Global hotkey registration and dispatch.
//!
//! Hotkeys are registered on a dedicated thread with its own message loop, so
//! the GUI thread never blocks and the hotkey keeps working while the control
//! panel is hidden. Each press is pushed into a channel that the main loop
//! drains on every frame.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_HOTKEY};

/// One hotkey press reported back to the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// The configured hotkey was pressed: toggle GameMode on or off.
    Toggle,
}

/// A registered hotkey: the Win32 id plus the action it triggers.
#[derive(Debug, Clone, Copy)]
struct Binding {
    id: i32,
    action: HotkeyAction,
}

/// Handle for a running hotkey thread.
///
/// Dropping it unregisters nothing by itself; call [`stop`] for a clean exit.
/// The thread is detached because it is driven by a window-less message loop.
pub struct HotkeyManager {
    receiver: Receiver<HotkeyAction>,
    stop: Sender<()>,
}

impl HotkeyManager {
    /// Register `modifiers + virtual_key` and start dispatching presses.
    ///
    /// When registration fails (the combination is reserved by Windows or taken
    /// by another process) the returned manager simply never fires; the app
    /// keeps working with the tray menu and control panel.
    pub fn start(modifiers: u32, virtual_key: u32) -> Self {
        let (action_tx, action_rx) = channel::<HotkeyAction>();
        let (stop_tx, stop_rx) = channel::<()>();

        thread::spawn(move || {
            let modifiers = normalize_modifiers(modifiers);
            let id = 0xA11CE;
            let registered = unsafe {
                RegisterHotKey(
                    std::ptr::null_mut(),
                    id,
                    modifiers | MOD_NOREPEAT,
                    virtual_key,
                ) != 0
            };

            if !registered {
                log::warn!(
                    "hotkey {modifiers:#x}+{virtual_key:#x} could not be registered; \
                     use the tray menu or control panel instead"
                );
                return;
            }

            let binding = Binding {
                id,
                action: HotkeyAction::Toggle,
            };

            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                loop {
                    if stop_rx.try_recv().is_ok() {
                        break;
                    }
                    // Drain pending messages without blocking forever so the stop
                    // channel is observed promptly.
                    if PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                        thread::sleep(std::time::Duration::from_millis(25));
                        continue;
                    }
                    if msg.message == WM_HOTKEY && msg.wParam as i32 == binding.id {
                        if action_tx.send(binding.action).is_err() {
                            break;
                        }
                    }
                }
                UnregisterHotKey(std::ptr::null_mut(), id);
            }
        });

        HotkeyManager {
            receiver: action_rx,
            stop: stop_tx,
        }
    }

    /// Non-blocking poll for a pending hotkey press.
    pub fn poll(&self) -> Option<HotkeyAction> {
        self.receiver.try_recv().ok()
    }

    /// Ask the hotkey thread to unregister and exit.
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Keeps only the modifier bits Windows accepts.
fn normalize_modifiers(modifiers: u32) -> u32 {
    modifiers & (MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN)
}

/// Maps a hotkey label such as `F10` to its virtual-key code.
#[allow(dead_code)] // the settings UI does not edit the hotkey yet; unit tests cover this mapping
pub fn vk_from_label(label: &str) -> Option<u32> {
    let upper = label.trim().to_ascii_uppercase();
    let vk = match upper.as_str() {
        "F1" => VK_F1 as u32,
        "F2" => VK_F2 as u32,
        "F3" => VK_F3 as u32,
        "F4" => VK_F4 as u32,
        "F5" => VK_F5 as u32,
        "F6" => VK_F6 as u32,
        "F7" => VK_F7 as u32,
        "F8" => VK_F8 as u32,
        "F9" => VK_F9 as u32,
        "F10" => VK_F10 as u32,
        "F11" => VK_F11 as u32,
        "F12" => VK_F12 as u32,
        single if single.len() == 1 => {
            let c = single.chars().next()?;
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase() as u32
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(vk)
}

use windows_sys::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_REMOVE};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_keys_map_to_virtual_codes() {
        assert_eq!(vk_from_label("F10"), Some(VK_F10 as u32));
        assert_eq!(vk_from_label("f1"), Some(VK_F1 as u32));
    }

    #[test]
    fn letters_and_garbage_are_handled() {
        assert_eq!(vk_from_label("g"), Some(0x47));
        assert_eq!(vk_from_label("not-a-key"), None);
        assert_eq!(vk_from_label(""), None);
    }

    #[test]
    fn modifiers_are_masked() {
        assert_eq!(normalize_modifiers(0xFFFF), MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN);
    }
}