# tools

Developer helpers for the GameMode project. These are run by hand from a stock
CPython 3 and are **not** part of the shipped build - the published binary comes
from `cargo build -p gm-gui --release`.

## Contents

| File | Purpose |
|------|---------|
| make_icons.py | regenerates the tray and application icons under `assets/` |

## make_icons.py

Draws the GameMode mark procedurally using only the standard library, so no
Pillow and no external source art are required.

Output files, with sizes 16, 24, 32, 48, 64, 128 and 256 px, plus curated 32x32
RGBA buffers used at runtime by `crate::icons`:

- `assets/gamemode-*.png` - the coloured mark
- `assets/tray-off.png` - the dimmed mark used while the optimiser is idle
- `assets/gamemode-32.rgba` / `assets/tray-off.rgba` - raw RGBA for the tray icon bytes
- `assets/gamemode.ico` - multi-resolution ICO for the executable resource

### Usage

```bash
python tools/make_icons.py
```

The script writes only into `assets/` and never modifies source files. It is
idempotent: running it twice produces identical output.
