//! Configuration model, defaults and TOML persistence.
//!
//! The config lives in `%APPDATA%\GameMode\config.toml`. Every field has a
//! conservative default so a missing or partial file still yields a valid setup.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::error::{GmError, GmResult};
use crate::prelude::{csidl, known_folder};

/// Which detection strategy decides that a game is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectMode {
    /// Only processes whose executable name is in the game list count.
    ListOnly,
    /// Only the foreground fullscreen application counts.
    ForegroundOnly,
    /// List, foreground fullscreen, or a process above the CPU threshold.
    Smart,
}

impl Default for DetectMode {
    fn default() -> Self {
        DetectMode::Smart
    }
}

impl DetectMode {
    pub fn label(&self) -> &'static str {
        match self {
            DetectMode::ListOnly => "Game list only",
            DetectMode::ForegroundOnly => "Foreground fullscreen only",
            DetectMode::Smart => "Smart (list + fullscreen + load)",
        }
    }
}

/// Windows power scheme to activate while GameMode is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerPlan {
    /// Do not touch the active power scheme.
    Keep,
    Balanced,
    HighPerformance,
    UltimatePerformance,
}

impl Default for PowerPlan {
    fn default() -> Self {
        PowerPlan::HighPerformance
    }
}

impl PowerPlan {
    pub fn label(&self) -> &'static str {
        match self {
            PowerPlan::Keep => "Leave unchanged",
            PowerPlan::Balanced => "Balanced",
            PowerPlan::HighPerformance => "High performance",
            PowerPlan::UltimatePerformance => "Ultimate performance",
        }
    }
}

/// How aggressively background (non-game) processes are demoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoostLevel {
    /// Only raise the game; never touch anything else.
    Off,
    /// Raise the game to HIGH and lower obvious background hogs.
    Balanced,
    /// Raise the game to HIGH, push every background process to IDLE and trim it.
    Aggressive,
    /// Above plus realtime-class game priority and CPU set pinning.
    Extreme,
}

impl Default for BoostLevel {
    fn default() -> Self {
        BoostLevel::Aggressive
    }
}

impl BoostLevel {
    pub fn label(&self) -> &'static str {
        match self {
            BoostLevel::Off => "Off (game boost only)",
            BoostLevel::Balanced => "Balanced",
            BoostLevel::Aggressive => "Aggressive",
            BoostLevel::Extreme => "Extreme",
        }
    }
    pub fn score(&self) -> u8 {
        match self {
            BoostLevel::Off => 0,
            BoostLevel::Balanced => 1,
            BoostLevel::Aggressive => 2,
            BoostLevel::Extreme => 3,
        }
    }
}

/// A user-registered game entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEntry {
    /// Executable file name, lower-cased (e.g. `cs2.exe`).
    pub exe: String,
    /// Optional friendly title shown in the UI.
    #[serde(default)]
    pub title: String,
    /// Optional full path used to verify a match.
    #[serde(default)]
    pub path: String,
    /// When true the entry is ignored during detection.
    #[serde(default)]
    pub disabled: bool,
}

impl GameEntry {
    pub fn new(exe: &str) -> Self {
        GameEntry {
            exe: exe.to_ascii_lowercase(),
            title: String::new(),
            path: String::new(),
            disabled: false,
        }
    }
    pub fn display(&self) -> String {
        if self.title.is_empty() {
            self.exe.clone()
        } else {
            format!("{} ({})", self.title, self.exe)
        }
    }
}

/// The complete application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,

    // ---- detection ---------------------------------------------------------
    pub detect_mode: DetectMode,
    /// Start GameMode automatically when a game is detected.
    pub auto_activate: bool,
    /// Seconds to wait after a game starts before enabling (lets launchers settle).
    pub activation_delay_secs: u64,
    /// Seconds without a detected game before GameMode disables itself.
    pub deactivation_grace_secs: u64,
    /// CPU percentage above which a process is suspicious enough to auto-detect.
    pub cpu_detect_threshold: f32,
    /// Extra executables to treat as games.
    pub games: Vec<GameEntry>,
    /// Processes that are never demoted, lowered or trimmed (lower-cased exe names).
    pub whitelist: Vec<String>,

    // ---- what GameMode does ------------------------------------------------
    pub boost_level: BoostLevel,
    pub power_plan: PowerPlan,
    /// Request a 0.5 ms system timer resolution (reduces input latency).
    pub high_timer_resolution: bool,
    /// Keep the system and display awake while GameMode is active.
    pub prevent_sleep: bool,
    /// Stop or pause a curated list of non-essential background services.
    pub manage_services: bool,
    /// Services GameMode stops while active (restored afterwards).
    pub services_to_stop: Vec<String>,
    /// Demote background processes that hold significant CPU.
    pub demote_background: bool,
    /// Trim the working set of demoted processes to free RAM for the game.
    pub trim_background_memory: bool,
    /// Reserve a private set of CPU cores for the game (keeps background off them).
    pub reserve_cores: bool,
    /// Number of logical CPUs to reserve for the game. 0 = automatic.
    pub reserved_core_count: usize,
    /// Ask the Windows scheduler to prefer this process (MMCSS-style audio boost).
    pub boost_audio_threads: bool,
    /// Disable Windows Game DVR / Game Bar background recording.
    pub disable_game_dvr: bool,
    /// Enable Hardware Accelerated GPU Scheduling.
    pub enable_hags: bool,
    /// Disable Nagle-style network throttling for game traffic.
    pub network_latency_tweaks: bool,
    /// Disable fullscreen optimisations for registered games.
    pub disable_fullscreen_optimizations: bool,
    /// Raise the menu-show delay so the game keeps focus.
    pub disable_menu_hover_delay: bool,
    /// Mouse/keyboard: disable Windows pointer acceleration while gaming (raw input path).
    pub keep_foreground_focus: bool,

    // ---- lifecycle ---------------------------------------------------------
    /// Launch minimised with no window shown.
    pub start_minimized: bool,
    /// Register GameMode to run at user logon.
    pub autostart: bool,
    /// Global hotkey to toggle GameMode (Win32 modifier bits + virtual key).
    pub hotkey_toggle_enabled: bool,
    pub hotkey_modifiers: u32,
    pub hotkey_virtual_key: u32,
    /// Restore every modified system setting when GameMode is disabled.
    pub restore_on_deactivate: bool,
    /// Restore settings and exit cleanly when Windows shuts down.
    pub restore_on_exit: bool,
    /// Show a toast/balloon when GameMode toggles.
    pub show_notifications: bool,
    /// Verbose logging to file.
    pub verbose_logging: bool,
    /// Poll interval for the process monitor, milliseconds.
    pub poll_interval_ms: u64,
    /// Hide the main window to the tray instead of exiting on close.
    pub close_to_tray: bool,
    pub theme_dark: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,

            detect_mode: DetectMode::Smart,
            auto_activate: true,
            activation_delay_secs: 3,
            deactivation_grace_secs: 20,
            cpu_detect_threshold: 25.0,
            games: default_games(),
            whitelist: default_whitelist(),

            boost_level: BoostLevel::Aggressive,
            power_plan: PowerPlan::HighPerformance,
            high_timer_resolution: true,
            prevent_sleep: true,
            manage_services: false,
            services_to_stop: default_services(),
            demote_background: true,
            trim_background_memory: true,
            reserve_cores: false,
            reserved_core_count: 0,
            boost_audio_threads: true,
            disable_game_dvr: true,
            enable_hags: true,
            network_latency_tweaks: true,
            disable_fullscreen_optimizations: false,
            disable_menu_hover_delay: true,
            keep_foreground_focus: true,

            start_minimized: false,
            autostart: false,
            hotkey_toggle_enabled: true,
            hotkey_modifiers: 0x0002 | 0x0004, // CTRL | SHIFT
            hotkey_virtual_key: 0x47,         // 'G'
            restore_on_deactivate: true,
            restore_on_exit: true,
            show_notifications: true,
            verbose_logging: false,
            poll_interval_ms: 1500,
            close_to_tray: true,
            theme_dark: true,
        }
    }
}

/// Well-known game executables so the app is useful before any configuration.
pub fn default_games() -> Vec<GameEntry> {
    const NAMES: &[(&str, &str)] = &[
        ("cs2.exe", "Counter-Strike 2"),
        ("csgo.exe", "Counter-Strike: GO"),
        ("valorant.exe", "VALORANT"),
        ("dota2.exe", "Dota 2"),
        ("r5apex.exe", "Apex Legends"),
        ("fortnite.exe", "Fortnite"),
        ("gta5.exe", "Grand Theft Auto V"),
        ("rdr2.exe", "Red Dead Redemption 2"),
        ("eldenring.exe", "Elden Ring"),
        ("witcher3.exe", "The Witcher 3"),
        ("cyberpunk2077.exe", "Cyberpunk 2077"),
        ("bf2042.exe", "Battlefield 2042"),
        ("rainbowsix.exe", "Rainbow Six Siege"),
        ("r6siege.exe", "Rainbow Six Siege"),
        ("overwatch.exe", "Overwatch 2"),
        ("destiny2.exe", "Destiny 2"),
        ("minecraft.exe", "Minecraft"),
        ("javaw.exe", "Minecraft (Java)"),
        ("league of legends.exe", "League of Legends"),
        ("leagueclient.exe", "League of Legends Client"),
        ("genshinimpact.exe", "Genshin Impact"),
        ("starfield.exe", "Starfield"),
        ("bg3.exe", "Baldur's Gate 3"),
        ("helldivers2.exe", "Helldivers 2"),
        ("pubg.exe", "PUBG: Battlegrounds"),
        ("rust.exe", "Rust"),
        ("escapefromtarkov.exe", "Escape from Tarkov"),
        ("warframe.exe", "Warframe"),
        ("diablo iv.exe", "Diablo IV"),
        ("forza horizon 5.exe", "Forza Horizon 5"),
        ("ac.exe", "Assetto Corsa"),
        ("acc.exe", "Assetto Corsa Competizione"),
        ("f1_24.exe", "F1 24"),
        ("nba2k24.exe", "NBA 2K24"),
        ("hl2.exe", "Source engine game"),
        ("tf2.exe", "Team Fortress 2"),
        ("left4dead2.exe", "Left 4 Dead 2"),
        ("portal2.exe", "Portal 2"),
        ("stardew valley.exe", "Stardew Valley"),
        ("terraria.exe", "Terraria"),
        ("factorio.exe", "Factorio"),
        ("satisfactory.exe", "Satisfactory"),
        ("cityskylines.exe", "Cities: Skylines"),
        ("cities2.exe", "Cities: Skylines II"),
        ("rocketleague.exe", "Rocket League"),
        ("marvel.exe", "Marvel Rivals"),
        ("thefinals.exe", "THE FINALS"),
        ("deadlock.exe", "Deadlock"),
        ("wow.exe", "World of Warcraft"),
        ("ffxiv_dx11.exe", "Final Fantasy XIV"),
        ("tarkov.exe", "Escape from Tarkov"),
        ("monsterhunterworld.exe", "Monster Hunter: World"),
        ("monsterhunterrise.exe", "Monster Hunter Rise"),
        ("sekiro.exe", "Sekiro"),
        ("darksouls3.exe", "Dark Souls III"),
        ("hollowknight.exe", "Hollow Knight"),
        ("palworld.exe", "Palworld"),
        ("enshrouded.exe", "Enshrouded"),
        ("spacemarine2.exe", "Warhammer 40K: Space Marine 2"),
        ("valorant-win64-shipping.exe", "VALORANT Shipping"),
        ("shipping.exe", "Unreal Engine shipping build"),
    ];
    NAMES
        .iter()
        .map(|(exe, title)| GameEntry {
            exe: (*exe).to_string(),
            title: (*title).to_string(),
            path: String::new(),
            disabled: false,
        })
        .collect()
}

/// Processes we never demote: core OS, audio, input, our own app and common launchers.
pub fn default_whitelist() -> Vec<String> {
    const KEEP: &[&str] = &[
        "system",
        "system idle process",
        "registry",
        "memory compression",
        "secure system",
        "smss.exe",
        "csrss.exe",
        "wininit.exe",
        "winlogon.exe",
        "services.exe",
        "lsass.exe",
        "svchost.exe",
        "dwm.exe",
        "explorer.exe",
        "sihost.exe",
        "taskhostw.exe",
        "fontdrvhost.exe",
        "ctfmon.exe",
        "audiodg.exe",
        "audiosrv",
        "conhost.exe",
        "runtimebroker.exe",
        "searchindexer.exe",
        "windowsaudio",
        "gamemode.exe",
        "nvcontainer.exe",
        "nvidia share.exe",
        "nvidia web helper.exe",
        "amdow.exe",
        "atiesrxx.exe",
        "rtss.exe",
        "msiafterburner.exe",
        "steam.exe",
        "steamwebhelper.exe",
        "epicgameslauncher.exe",
        "battle.net.exe",
        "riotclientservices.exe",
        "discord.exe",
        "obs64.exe",
    ];
    KEEP.iter().map(|s| s.to_string()).collect()
}

/// Non-essential services that are reasonable to pause while gaming.
pub fn default_services() -> Vec<String> {
    const SVCS: &[&str] = &[
        "SysMain",          // Superfetch
        "WSearch",          // Windows Search indexer
        "DiagTrack",        // Connected User Experiences and Telemetry
        "dmwappushservice",  // WAP push
        "MapsBroker",
        "Fax",
        "RemoteRegistry",
        "WerSvc",
        "Spooler",
        "TabletInputService",
    ];
    SVCS.iter().map(|s| s.to_string()).collect()
}

impl Config {
    /// Directory that holds the configuration, log and snapshot files.
    pub fn data_dir() -> PathBuf {
        if let Ok(custom) = std::env::var("GAMEMODE_DATA_DIR") {
            if !custom.trim().is_empty() {
                return PathBuf::from(custom);
            }
        }
        known_folder(csidl::APP_DATA)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("GameMode")
    }

    pub fn config_path() -> PathBuf {
        Self::data_dir().join("config.toml")
    }

    pub fn ensure_dirs() -> GmResult<PathBuf> {
        let dir = Self::data_dir();
        std::fs::create_dir_all(&dir).map_err(GmError::Io)?;
        Ok(dir)
    }

    /// Load the configuration, creating a default file when none exists.
    pub fn load() -> GmResult<Self> {
        Self::ensure_dirs()?;
        let path = Self::config_path();
        if !path.exists() {
            let cfg = Config::default();
            let _ = cfg.save();
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(&path).map_err(GmError::Io)?;
        match toml::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                cfg.validate();
                Ok(cfg)
            }
            Err(e) => {
                // Keep the broken file for inspection, then fall back to defaults.
                let backup = path.with_extension("toml.broken");
                let _ = std::fs::copy(&path, &backup);
                log::warn!("config parse failed ({e}); defaults restored, broken copy at {backup:?}");
                let cfg = Config::default();
                let _ = cfg.save();
                Ok(cfg)
            }
        }
    }

    /// Persist to `config.toml`, writing atomically through a temporary file.
    pub fn save(&self) -> GmResult<()> {
        Self::ensure_dirs()?;
        let path = Self::config_path();
        let text = toml::to_string_pretty(self)
            .map_err(|e| GmError::Config(format!("cannot serialise config: {e}")))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(GmError::Io)?;
        std::fs::rename(&tmp, &path).or_else(|_| {
            std::fs::copy(&tmp, &path)?;
            std::fs::remove_file(&tmp)
        })?;
        Ok(())
    }

    /// Clamp every numeric field into a sane range.
    pub fn validate(&mut self) {
        self.version = 1;
        self.activation_delay_secs = self.activation_delay_secs.min(120);
        self.deactivation_grace_secs = self.deactivation_grace_secs.min(600);
        self.cpu_detect_threshold = self.cpu_detect_threshold.clamp(1.0, 99.0);
        self.poll_interval_ms = self.poll_interval_ms.clamp(250, 15000);
        self.reserved_core_count = self.reserved_core_count.min(64);
        for g in &mut self.games {
            g.exe = g.exe.to_ascii_lowercase();
        }
        for w in &mut self.whitelist {
            *w = w.to_ascii_lowercase();
        }
        self.whitelist.push(crate::prelude::exe_name().to_ascii_lowercase());
        self.whitelist.sort();
        self.whitelist.dedup();
    }

    /// True when `exe` (lower-cased) appears in the whitelist.
    pub fn is_whitelisted(&self, exe: &str) -> bool {
        let e = exe.to_ascii_lowercase();
        self.whitelist.iter().any(|w| *w == e)
    }

    /// True when `exe` matches an enabled game entry.
    pub fn is_game_name(&self, exe: &str) -> bool {
        let e = exe.to_ascii_lowercase();
        self.games
            .iter()
            .any(|g| !g.disabled && (g.exe == e || (!g.path.is_empty() && g.path.to_ascii_lowercase().ends_with(&e))))
    }

    /// Register a game executable, returning false when it already exists.
    pub fn add_game(&mut self, exe: &str, title: &str) -> bool {
        let e = exe.trim().to_ascii_lowercase();
        if e.is_empty() || self.games.iter().any(|g| g.exe == e) {
            return false;
        }
        self.games.push(GameEntry {
            exe: e,
            title: title.trim().to_string(),
            path: String::new(),
            disabled: false,
        });
        true
    }

    pub fn remove_game(&mut self, exe: &str) -> bool {
        let e = exe.to_ascii_lowercase();
        let before = self.games.len();
        self.games.retain(|g| g.exe != e);
        self.games.len() != before
    }

    /// Human-readable hotkey label, e.g. `Ctrl+Shift+G`.
    pub fn hotkey_label(&self) -> String {
        if !self.hotkey_toggle_enabled {
            return "disabled".into();
        }
        let mut parts = Vec::new();
        if self.hotkey_modifiers & 0x0001 != 0 {
            parts.push("Alt".to_string());
        }
        if self.hotkey_modifiers & 0x0002 != 0 {
            parts.push("Ctrl".to_string());
        }
        if self.hotkey_modifiers & 0x0004 != 0 {
            parts.push("Shift".to_string());
        }
        if self.hotkey_modifiers & 0x0008 != 0 {
            parts.push("Win".to_string());
        }
        let vk = self.hotkey_virtual_key;
        let key = match vk {
            0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default(),
            0x70..=0x7B => format!("F{}", vk - 0x6F),
            0x20 => "Space".into(),
            0x09 => "Tab".into(),
            0x0D => "Enter".into(),
            0x1B => "Esc".into(),
            0x2C => "PrtSc".into(),
            other => format!("0x{other:02X}"),
        };
        parts.push(key);
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("serialise");
        let back: Config = toml::from_str(&text).expect("deserialise");
        assert_eq!(back.games.len(), cfg.games.len());
        assert_eq!(back.boost_level, cfg.boost_level);
        assert_eq!(back.power_plan, cfg.power_plan);
    }

    #[test]
    fn validate_clamps_and_normalises() {
        let mut cfg = Config::default();
        cfg.poll_interval_ms = 1;
        cfg.cpu_detect_threshold = 500.0;
        cfg.games.push(GameEntry::new("MYGAME.EXE"));
        cfg.validate();
        assert_eq!(cfg.poll_interval_ms, 250);
        assert_eq!(cfg.cpu_detect_threshold, 99.0);
        assert!(cfg.games.iter().any(|g| g.exe == "mygame.exe"));
        assert!(cfg.is_game_name("MYGAME.EXE"));
    }

    #[test]
    fn game_add_remove_works() {
        let mut cfg = Config::default();
        assert!(cfg.add_game("TestGame.exe", "Test"));
        assert!(!cfg.add_game("testgame.EXE", "dup"));
        assert!(cfg.is_game_name("testgame.exe"));
        assert!(cfg.remove_game("TESTGAME.exe"));
        assert!(!cfg.is_game_name("testgame.exe"));
    }

    #[test]
    fn whitelist_matching() {
        let cfg = Config::default();
        assert!(cfg.is_whitelisted("EXPLORER.EXE"));
        assert!(!cfg.is_whitelisted("randommalware.exe"));
    }

    #[test]
    fn hotkey_label_renders() {
        let cfg = Config::default();
        assert_eq!(cfg.hotkey_label(), "Ctrl+Shift+G");
    }
}
