<div align="center">

<img src="assets/gamemode-128.png" alt="GameMode" width="128">

[![Rust](https://img.shields.io/badge/Rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)
[![Windows](https://img.shields.io/badge/Windows-10%2F11-blue.svg)](https://www.microsoft.com/windows)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Build](https://img.shields.io/badge/Build-passing-brightgreen.svg)]()

</div>

# 🎮 GameMode — Gaming Optimiser for Windows

A professional, pure-Rust system-tray application that gives your active game **first priority** on CPU, memory, disk and power so it runs as smoothly as possible — even when other apps are fighting for resources. Every change is journaled and restored exactly when you turn it off. Nothing is left behind.

---

## 🎯 What it does

Turn GameMode **ON**, and every game you play gets real-time optimisation with exact, automatic restore when you turn it **OFF**.

| Area | What GameMode optimises |
|------|-------------------------|
| **CPU priority** | Raises your game's process + thread priority so the Windows scheduler always runs it first, ahead of background workloads. |
| **Core reservation** | CPU affinity masking reserves cores for the game; heavy background work cannot starve it. |
| **Power plan** | Switches to High Performance (100 % processor floor & ceiling) so clocks stay up. |
| **Timer resolution** | Raises system timer resolution to 0.5 ms for smooth frame pacing. |
| **Sleep inhibition** | System and display stay awake while the game runs. |
| **Background demotion** | Optional de-prioritisation of background processes & services. |
| **Registry tweaks** | Optional MMCSS / Game DVR / fullscreen-optimisation / HAGS gaming tweaks. |

Every change is journaled into a **snapshot** and restored exactly on deactivate or quit — nothing is left behind.

---

## 🔒 Locked / Manual Mode (NEW)

GameMode now supports a **locked mode** where you pick one specific executable and GameMode stays locked to it — ignoring all automatic detection. Useful for launchers, unusual anti-cheat setups, or when you just want *that one game* boosted and nothing else.

```bash
# Lock to a specific game (stays active until you unlock)
gamemode.exe --lock-game cs2.exe

# Or specify the full path
gamemode.exe --lock-game "C:\SteamLibrary\steamapps\common\Cyberpunk 2077\Cyberpunk2077.exe"

# Unlock and return to automatic detection
gamemode.exe --unlock
```

In locked mode:
- GameMode activates **immediately**, bypassing the normal arming delay.
- It applies every optimisation **only** to the chosen executable.
- If the process isn't running yet, GameMode waits and re-applies the moment it appears.
- State persists across restarts (saved to `%APPDATA%\GameMode\config.toml`).

---

## 📊 Real-Time Dashboard (NEW)

The control panel now includes a **live performance dashboard** with real GPU, CPU and RAM gauges:

- **CPU** — system-wide utilisation from `GetSystemTimes` (no process-table approximation).
- **GPU** — **real** utilisation from Windows Performance Counters (`\GPU Engine(*)\Utilization Percentage`), not a CPU proxy. Works on NVIDIA, AMD, Intel; shows engine count.
- **RAM** — total / available / used, plus a "saved" figure showing how much background working set was trimmed.
- **Sparklines** — 120-sample history rings on each tile so you see trends, not just instantaneous values.
- **Top-process tables** — heaviest CPU and RAM consumers, with per-process RAM in MB (from `K32GetProcessMemoryInfo`), and a "game" badge so you can spot the target at a glance.

<p align="center">
  <img src="assets/dashboard-preview.png" alt="GameMode dashboard" width="720">
</p>

---

## 📋 Instant Activation Report

The moment you enable GameMode, the panel shows a readable list of **everything it did**:

```
✅ Privileges        →  6 debug privileges enabled
✅ Timer resolution  →  raised to 0.50 ms
✅ Sleep inhibition  →  system & display wake locks acquired
✅ Power plan        →  High Performance activated
✅ Background demote →  14 processes lowered to IDLE priority
✅ Services          →  3 non-essential services paused
✅ Registry tweaks   →  MMCSS / Game DVR / HAGS applied
```

Each line is colour-coded (green ✅ applied, amber ➖ skipped, red ❌ failed). The footer summarises resources freed: `CPU 12% freed · RAM 420 MB freed · GPU engines 4 active`.

---

## 📦 Install

### Pre-built release

Download [`gamemode-1.0.0-windows-x64.zip`](https://github.com/Michaelunkai/gamemode/releases/download/v1.0.0/gamemode-1.0.0-windows-x64.zip) from the [Releases](https://github.com/Michaelunkai/gamemode/releases) page. No installer — unzip and run `gamemode.exe`. It installs itself into the tray and starts with Windows if you enable autostart.

### Build from source

```bash
# Requires Rust 1.78+ (stable-x86_64-pc-windows-gnu)
git clone https://github.com/Michaelunkai/gamemode.git
cd gamemode
cargo build --release
```

The release binary lands at `target/x86_64-pc-windows-gnu/release/gamemode.exe`; build with a project-local output if your environment holds a lock on the shared target directory:

```bash
cargo build -p gm-gui --release --target-dir ./target/release-local
```

---

## 🚀 Usage

Run the binary. A GameMode icon appears in your system tray and stays there as long as the app runs.

```
gamemode.exe --tray           # start hidden in the tray (no window)
gamemode.exe --headless --hold 60   # activate for 60 seconds, then restore
gamemode.exe --lock-game "Game.exe" # lock to a specific executable
gamemode.exe --unlock         # return to automatic mode
gamemode.exe quit             # IPC: deactivate & fully stop a running instance
```

**Tray menu** — toggle On/Off, open the control panel, toggle autostart, quit.  
**Control panel** — live dashboard, detected games, boost level, power plan, timer resolution, log viewer, activation report.

### Behaviour contract

- The tray icon exists as long as the process runs, in every state.
- Quitting from the tray **fully stops** GameMode: the session is deactivated (every machine change restored) and only then does the process exit.
- Single-instance: only one `gamemode.exe` owns the machine; later launches forward their command and exit.
- Locked mode persists in config (`locked_mode = true`, `locked_game_exe = "game.exe"`).

---

## 🏗️ Architecture

Pure Rust workspace — two crates:

- **`gm-core`** — detection, optimisation engine, durable snapshots, Win32/NT API wrappers, PDH performance counters, real-time telemetry ring buffers.
- **`gm-gui`** — thin binary: system tray, global hotkey, egui control panel, named-pipe IPC, autostart, beautiful dashboard.

[Read the full architecture → docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

---

## ✅ Verification

Verified on Windows 11, AMD Ryzen 7 9800X3D (8C/16T), Admin:

- Release build compiles clean; `gamemode.exe` ~6.4 MB optimised.
- Headless activate → High Performance plan → 0.5 ms timer → sleep inhibition → full restore: `RESULT:OK`.
- Locked mode activates instantly, waits for target process, re-applies on spawn, persists across restarts.
- Tray mode runs 1 background process; IPC `quit` restores state and exits to 0 processes.
- Second launch forwards commands via named pipe (no competing optimiser).
- GPU counters return real utilisation on NVIDIA/AMD/Intel; falls back gracefully if PDH unavailable.

[Full verification report → docs/VERIFICATION.md](docs/VERIFICATION.md)

---

## 📄 License

MIT

<div align="center">
<sub>Built with Rust and a stubborn refusal to leave systems untidy.</sub>
</div>