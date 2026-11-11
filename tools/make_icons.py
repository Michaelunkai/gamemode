"""Generate the GameMode icon set (multi-size PNG + ICO) with no dependencies.

The mark is a rounded tile with an indigo -> cyan gradient and a white
lightning bolt. Everything is rasterised by hand and encoded with zlib, so the
script runs on a stock CPython with no third-party packages.
"""

import os
import struct
import zlib

OUT = r"F:\study\projects\games\gamemode\assets"
os.makedirs(OUT, exist_ok=True)

# Bolt outline in unit coordinates, clockwise.
BOLT = [
    (0.60, 0.08),
    (0.28, 0.56),
    (0.455, 0.56),
    (0.40, 0.92),
    (0.73, 0.43),
    (0.545, 0.43),
]

TOP = (0x1B, 0x1F, 0x3B)      # deep indigo
BOTTOM = (0x0A, 0x9A, 0xD6)   # electric blue
ACCENT = (0x22, 0xE0, 0xC8)   # cyan edge light
WHITE = (0xFF, 0xFF, 0xFF)


def _lerp(a, b, t):
    return a + (b - a) * t


def _mix(c1, c2, t):
    return tuple(_lerp(c1[i], c2[i], t) for i in range(3))


def _in_poly(px, py, poly):
    inside = False
    n = len(poly)
    for i in range(n):
        x1, y1 = poly[i]
        x2, y2 = poly[(i + 1) % n]
        if (y1 > py) != (y2 > py):
            xin = (x2 - x1) * (py - y1) / (y2 - y1) + x1
            if px < xin:
                inside = not inside
    return inside


def _round_rect_alpha(fx, fy, radius, px):
    """Anti-aliased coverage of a rounded 0..1 square with the given corner radius.

    Computed from the signed distance to the rounded rectangle, so the straight
    edges keep full coverage and only the corners curve. `px` is one pixel in
    unit coordinates and drives the anti-aliasing ramp.
    """
    # Distance from the centre, folded into the positive quadrant.
    qx = abs(fx - 0.5) - (0.5 - radius)
    qy = abs(fy - 0.5) - (0.5 - radius)
    ox = max(qx, 0.0)
    oy = max(qy, 0.0)
    outside = (ox * ox + oy * oy) ** 0.5
    dist = outside + min(max(qx, qy), 0.0) - radius
    # dist < 0 inside, > 0 outside; ramp across roughly one pixel.
    return min(1.0, max(0.0, 0.5 - dist / px))


def _bolt_alpha(fx, fy):
    """Anti-aliased coverage of the lightning bolt (4x4 supersampling)."""
    hits = 0
    for sy in range(4):
        for sx in range(4):
            px = fx + (sx + 0.5) / (4.0 * SIZE_HINT)
            py = fy + (sy + 0.5) / (4.0 * SIZE_HINT)
            if _in_poly(px, py, BOLT):
                hits += 1
    return hits / 16.0


SIZE_HINT = 256


def render(size):
    """Return RGBA rows for one icon size."""
    global SIZE_HINT
    SIZE_HINT = size
    rows = []
    radius = 0.24
    for y in range(size):
        row = bytearray()
        for x in range(size):
            fx = (x + 0.5) / size
            fy = (y + 0.5) / size
            a = _round_rect_alpha(fx, fy, radius, 1.0 / size)
            if a <= 0.0:
                row += bytes((0, 0, 0, 0))
                continue
            # Vertical gradient with a diagonal sheen.
            t = min(1.0, max(0.0, fy * 0.85 + fx * 0.15))
            base = _mix(TOP, BOTTOM, t)
            # Cyan rim light near the top-left edge.
            edge = max(0.0, 1.0 - min(fx, fy) / 0.18)
            base = _mix(base, ACCENT, edge * 0.35)
            ba = _bolt_alpha(fx, fy)
            col = _mix(base, WHITE, ba)
            r, g, b = (int(round(c)) for c in col)
            alpha = int(round(255 * a))
            row += bytes((r, g, b, alpha))
        rows.append(bytes(row))
    return rows


def png_bytes(size):
    rows = render(size)
    raw = b"".join(b"\x00" + row for row in rows)

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def ico_bytes(sizes):
    """Build a PNG-compressed ICO (Vista+ format) containing every size."""
    images = [(s, png_bytes(s)) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(images))
    entries = b""
    offset = 6 + 16 * len(images)
    body = b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        body += data
        offset += len(data)
    return header + entries + body


def main():
    sizes = [16, 24, 32, 48, 64, 128, 256]
    for s in sizes:
        path = os.path.join(OUT, f"gamemode-{s}.png")
        with open(path, "wb") as fh:
            fh.write(png_bytes(s))
        print("wrote", path)

    ico_path = os.path.join(OUT, "gamemode.ico")
    with open(ico_path, "wb") as fh:
        fh.write(ico_bytes(sizes))
    print("wrote", ico_path, os.path.getsize(ico_path), "bytes")

    # Tray icons: 32px for the active state, 32px desaturated for idle.
    idle = bytearray(b"".join(render(32)))
    for y in range(32):
        for x in range(32):
            i = (y * 32 + x) * 4
            if idle[i + 3]:
                # Desaturate and dim to grey for the "off" tray state.
                lum = int(0.299 * idle[i] + 0.587 * idle[i + 1] + 0.114 * idle[i + 2])
                lum = int(lum * 0.62 + 40)
                idle[i] = idle[i + 1] = idle[i + 2] = min(255, lum)
    raw = b"".join(b"\x00" + bytes(idle[y * 128:(y + 1) * 128]) for y in range(32))

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    ihdr = struct.pack(">IIBBBBB", 32, 32, 8, 6, 0, 0, 0)
    tray_off = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
                + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
    with open(os.path.join(OUT, "tray-off.png"), "wb") as fh:
        fh.write(tray_off)
    print("wrote tray-off.png")

    # Raw RGBA dumps so the tray module can `include_bytes!` them.
    with open(os.path.join(OUT, "gamemode-32.rgba"), "wb") as fh:
        fh.write(bytes(b"".join(render(32))))
    with open(os.path.join(OUT, "tray-off.rgba"), "wb") as fh:
        fh.write(bytes(idle))
    print("wrote raw RGBA tray assets")


if __name__ == "__main__":
    main()
