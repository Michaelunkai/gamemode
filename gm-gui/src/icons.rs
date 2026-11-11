//! Tray icons, compiled into the binary.

use tray_icon::Icon;

fn build(bytes: &[u8]) -> Option<Icon> {
    Icon::from_rgba(bytes.to_vec(), 32, 32).ok()
}

/// The coloured mark, shown while GameMode is optimising.
pub fn active() -> Icon {
    build(include_bytes!("../../assets/gamemode-32.rgba")).unwrap_or_else(fallback)
}

/// The dimmed mark, shown while GameMode is idle.
pub fn idle() -> Icon {
    build(include_bytes!("../../assets/tray-off.rgba")).unwrap_or_else(fallback)
}

fn fallback() -> Icon {
    Icon::from_rgba(vec![120, 120, 120, 255].repeat(1024), 32, 32).expect("fallback icon")
}