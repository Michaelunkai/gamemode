<div align="center">

<img src="assets/gamemode-128.png" alt="GameMode" width="128">

[![Rust](https://img.shields.io/badge/Rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)
[![Windows](https://img.shields.io/badge/Windows-10%2F11-0078D6.svg)](https://www.microsoft.com/windows)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Build](https://img.shields.io/badge/build-passing-brightgreen.svg)]()

# 🎮 GameMode — Gaming Optimiser for Windows

**First-priority gaming performance for Windows, written in pure Rust.**

A system-tray optimiser that gives your active game the CPU, memory, storage and
power it needs — and restores everything the moment you turn it off.

[What it does](#-what-it-does) · [Install](#-install) · [Usage](#-usage) · [Architecture](#-architecture) · [Verification](docs/VERIFICATION.md)

</div>

---

## 🎯 What it does

Turn GameMode **ON**, and every game you play gets real-time optimisation with exact,
automatic restore when you turn it **OFF**.

| Area | What GameMode optimises |
|------|-------------------------|
| **CPU priority** | Raises your game’s process + thread priority so the Windows scheduler always runs it first, ahead of background workloads. |
| **Core reservation** | CPU affinity masking reserves cores for the game; heavy background work cannot starve it. |
| **Power plan** | Switches to High Performance (100 % processor floor & ceiling) so clocks stay up. |
| **Timer resolution** | Raises system timer resolution to 0.5 ms for smooth frame pacing. |
| **Sleep inhibition** | System and display stay awake while the game runs. |
| **Background demotion** | Optional de-prioritisation of background processes & services. |
| **Registry tweaks** | Optional MMCSS / Game DVR / fullscreen-optimisation / HAGS gaming tweaks. |

Every change is journaled into a **snapshot** and restored exactly on deactivate or quit — nothing is left behind.

---

## 📦 Install

### Pre-built release

Download [`gamemode.exe`] from the [Releases](https://github.com/Michaelunkai/gamemode/releases) page. No installer — just run it. It installs itself into the tray and starts with Windows if you enable autostart.

### Build from source

```bash
# Requires Rust 1.78+ (stable-x86_64-pc-windows-gnu)
git clone https://github.com/Michaelunkai/gamemode.git
cd gamemode
cargo build --release
```

The release binary lands at `target/release/x86_64-pc-windows-gnu/release/gamemode.exe`.

---

## 🚀 Usage

Run the binary. A GameMode icon appears in your system tray and stays there as long as the app runs.

```
gamemode.exe --tray           # start hidden in the tray (no window)
gamemode.exe --headless --hold 60   # activate for 60 seconds, then restore
gamemode.exe quit             # IPC: deactivate & fully stop a running instance
```

**Tray menu** — toggle On/Off, open the control panel, toggle autostart, quit.  
**Control panel** — live status, detected games, boost level, power plan, timer resolution, log viewer.

### Behaviour contract
- The tray icon exists as long as the process runs, in every state.
- Quitting from the tray **fully stops** GameMode: the session is deactivated (every machine change restored) and only then does the process exit.
- Single-instance: only one `gamemode.exe` owns the machine; later launches forward their command and exit.

---

## 🏗️ Architecture

Pure Rust workspace — two crates:

- **`gm-core`** — detection, optimisation engine, durable snapshots, Win32/NT API wrappers.
- **`gm-gui`** — thin binary: system tray, global hotkey, egui control panel, named-pipe IPC, autostart.

[Read the full architecture → docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

---

## ✅ Verification

Verified on Windows 11, AMD Ryzen 7 9800X3D (8C/16T), Admin:

- Release build compiles clean; `gamemode.exe` 6.4 MB optimised.
- Headless activate → High Performance plan → 0.5 ms timer → sleep inhibition → full restore: `RESULT:OK`.
- Tray mode runs 1 background process; IPC `quit` restores state and exits to 0 processes.
- Second launch forwards commands via named pipe (no competing optimiser).

[Full verification report → docs/VERIFICATION.md](docs/VERIFICATION.md)

---

## 📄 License

MIT

<div align="center">
<sub>Built with Rust and a stubborn refusal to leave systems untidy.</sub>
</div>
