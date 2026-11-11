//! System-tray icon: menu, tooltip and lifetime.
//!
//! The icon exists for as long as the process runs, in every state. Its menu is
//! the primary control surface, and `Quit` performs a full teardown: the session
//! is deactivated (every machine change restored) before the process exits.

use std::sync::Arc;

use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use gm_core::session::SessionState;

use crate::app::{AppState, Command};
use crate::icons;

/// Stable menu item ids. They are parsed back in `tray::handle_menu_event`.
pub const ID_TOGGLE: &str = "gm.toggle";
pub const ID_SHOW: &str = "gm.show";
pub const ID_AUTOSTART: &str = "gm.autostart";
pub const ID_SETTINGS: &str = "gm.settings";
pub const ID_QUIT: &str = "gm.quit";

/// Owns the live tray icon and its menu handles.
pub struct Tray {
    pub icon: TrayIcon,
    toggle: CheckMenuItem,
    autostart: CheckMenuItem,
}

impl Tray {
    /// Create the tray icon with its menu.
    pub fn new(app: &Arc<AppState>) -> Result<Self, tray_icon::Error> {
        let menu = Menu::new();

        let toggle = CheckMenuItem::with_id(ID_TOGGLE, "GameMode active", true, false, None);
        let header = MenuItem::with_id(ID_SHOW, "Open control panel", true, None);
        let settings = MenuItem::with_id(ID_SETTINGS, "Edit settings file", true, None);
        let auto = CheckMenuItem::with_id(
            ID_AUTOSTART,
            "Start with Windows",
            true,
            crate::autostart::is_enabled(),
            None,
        );
        let quit = MenuItem::with_id(ID_QUIT, "Quit GameMode", true, None);

        menu.append_items(&[
            &header,
            &PredefinedMenuItem::separator(),
            &toggle,
            &PredefinedMenuItem::separator(),
            &auto,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok();

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("GameMode — off")
            .with_icon(icons::idle())
            .build()?;

        let _ = app;
        Ok(Tray {
            icon,
            toggle,
            autostart: auto,
        })
    }

    /// Reflect the session state in the menu and the tooltip.
    ///
    /// Called whenever the state changes; cheap enough to call every frame
    /// because both setters are no-ops when the value is unchanged.
    pub fn sync(&self, state: SessionState, game: &str, hotkey: &str) {
        let on = state.is_optimising();
        if self.toggle.is_checked() != on {
            self.toggle.set_checked(on);
        }
        let label = if on { "GameMode active" } else { "Activate GameMode" };
        if self.toggle.text() != label {
            self.toggle.set_text(label);
        }
        let tip = match (on, game.is_empty()) {
            (true, false) => format!("GameMode ON — {game} [{hotkey}]"),
            (true, true) => format!("GameMode ON [{hotkey}]"),
            (false, _) => format!("GameMode off [{hotkey}]"),
        };
        let _ = self.icon.set_tooltip(Some(&tip));
        let _ = self.icon.set_icon(Some(if on { icons::active() } else { icons::idle() }));
    }

    /// Refresh the autostart check mark from the registry.
    pub fn sync_autostart(&self) {
        let enabled = crate::autostart::is_enabled();
        if self.autostart.is_checked() != enabled {
            self.autostart.set_checked(enabled);
        }
    }
}

/// Translate a menu event id into the command the main loop should run.
///
/// Returns `None` for ids that belong to another component.
pub fn command_for(id: &str) -> Option<Command> {
    match id {
        ID_TOGGLE => Some(Command::Toggle),
        ID_SHOW => Some(Command::ShowWindow),
        ID_QUIT => Some(Command::Quit),
        ID_SETTINGS => Some(Command::OpenConfig),
        ID_AUTOSTART => Some(Command::SetAutostart(!crate::autostart::is_enabled())),
        _ => None,
    }
}