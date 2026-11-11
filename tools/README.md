# tools

## Overview
One-off developer scripts for the GameMode project. They are run by hand from a stock CPython and are not part of the shipped build. `make_icons.py` generates the icon assets; `patch1.py` and `patch2.py` are historical text patches against `gm-gui/src/single_instance.rs`.

## Interface
- `make_icons.py`: dependency-free icon generator (stdlib `os`, `struct`, `zlib`).
  - `render(size)`: returns RGBA rows for one square icon size.
  - `png_bytes(size)`: encodes a size as an RGBA PNG.
  - `ico_bytes(sizes)`: builds a PNG-compressed ICO containing every given size.
  - `main()`: writes the full asset set (entry point under `__main__`).
  - Module constant `OUT` (`F:\study\projects\games\gamemode\assets`) is the hard-coded output directory.
- `patch1.py`: appends `verb_from_args` (parses `on/enable/start`, `off/disable/stop`, `toggle`, `show/open/panel`, `quit/exit`, `status` into `Verb`; takes the arguments without the program name) to `gm-gui/src/single_instance.rs`.
- `patch2.py`: removes the `instance_running` helper from `gm-gui/src/single_instance.rs`.
- `patch_open.py`: empty file (0 bytes); no behaviour.

## Invariants
- `make_icons.py` sizes are `[16, 24, 32, 48, 64, 128, 256]`; in the ICO, size 256 is encoded as dimension byte `0`.
- Tray assets are 32x32; `tray-off` is the desaturated, dimmed render of the 32px icon.
- `patch1.py` is idempotent: it skips when `fn verb_from_args` is already present.
- `patch1.py` and `patch2.py` resolve `BASE` as the parent of this directory and `chdir` there, so targets are relative to the repository root.
- `patch2.py` matches the helper text `"...already serves the control pipe."` (LF or CRLF), writes the file only when something was removed, and otherwise prints `single_instance: helper not present, nothing to do`. The helper is already gone from `single_instance.rs`, so it is a no-op today.

## Side Effects
- `make_icons.py` creates `OUT` if missing and writes into it: `gamemode-{16,24,32,48,64,128,256}.png`, `gamemode.ico`, `tray-off.png`, `gamemode-32.rgba`, `tray-off.rgba`. Existing files are overwritten. It prints one line per file.
- `patch1.py` and `patch2.py` change the process working directory and may rewrite `gm-gui/src/single_instance.rs` in place (UTF-8, no newline translation); both skip the write when there is nothing to change.

## Verification
- No test command applies to these one-off scripts.
- Running `python make_icons.py` regenerates the assets and prints each file it wrote.
