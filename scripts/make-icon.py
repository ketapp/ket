#!/usr/bin/env python3
"""Generates ket's app icon: the Dirac ket, |⟩.

The name is quantum-mechanical, and so is the tool's central verb. A ket is a
state vector, and a state vector in superposition is many possible outcomes at
once until measurement collapses it to one. That is exactly what ket does — N
agents on one task in parallel worktrees, then `ket collapse` keeps a single
one. Drawing the notation is drawing the idea.

Written with no image-library dependency, because the alternative is adding
Pillow to a Rust project's toolchain in order to draw two shapes. PNG is a
simple enough container to emit directly: a header, one zlib-compressed IDAT,
an IEND.

Usage:
    python3 scripts/make-icon.py           # writes assets/ket.png and assets/ket.icns
    python3 scripts/make-icon.py --phone   # writes the phone app's icons and splash
                                           # into mobile/app/assets, and nothing else
"""

from __future__ import annotations

import argparse
import json
import pathlib
import struct
import subprocess
import sys
import zlib

SIZE = 1024

# Matches the theme in crates/ket-ui/src/main.rs, so the icon and the window agree.
BG = (0x1B, 0x1F, 0x23)
INK = (0x7E, 0xE0, 0x8A)

# The phone app's backdrop (`backgroundColor` in mobile/app/app.json). The mark
# keeps the same INK.
PHONE_BG = (0x0D, 0x0C, 0x0B)
WHITE = (0xFF, 0xFF, 0xFF)

# The mark is two filled shapes, drawn the way ⟩ sets in a maths face: a bar
# and a bracket with a 60° apex, mitred to a sharp point, every end cut flat.
# Diagonals read heavier than verticals at equal width, so the bracket is a
# little thinner than the bar. The numbers are in a 1024 box and are shared
# with the logo canvas; keep them in step.
BAR = ((301, 272), (401, 272), (401, 752), (301, 752))
BRACKET = (
    (437.8, 272), (544.3, 272), (722.3, 512),
    (544.3, 752), (437.8, 752), (539.7, 512),
)
MARK_LEFT, MARK_RIGHT = 301, 722.3
MARK_TOP, MARK_BOTTOM = 272, 752

# macOS rounds app icons to a superellipse at roughly 22.4% of the edge. A
# circular radius lands within a few pixels at this size and needs no curve
# solver.
CORNER = int(SIZE * 0.224)

# Edge softness in pixels. The mark is downscaled to 16px by `sips`, which does
# its own filtering, but the 512 and 1024 variants are shown at full size and
# hard edges read as jagged there.
FEATHER = 1.6


def clamp01(value: float) -> float:
    """Clamps to the unit interval."""
    return 0.0 if value < 0.0 else 1.0 if value > 1.0 else value


def point_in_polygon(px: float, py: float, poly) -> bool:
    """Even-odd test."""
    inside = False
    n = len(poly)
    for i in range(n):
        ax, ay = poly[i]
        bx, by = poly[(i + 1) % n]
        if (ay > py) != (by > py):
            x_at = ax + (py - ay) * (bx - ax) / (by - ay)
            if px < x_at:
                inside = not inside
    return inside


def distance_to_segment(px: float, py: float, ax: float, ay: float, bx: float, by: float) -> float:
    """Shortest distance from a point to a line segment."""
    dx, dy = bx - ax, by - ay
    length_sq = dx * dx + dy * dy
    if length_sq == 0.0:
        return ((px - ax) ** 2 + (py - ay) ** 2) ** 0.5

    t = clamp01(((px - ax) * dx + (py - ay) * dy) / length_sq)
    cx, cy = ax + t * dx, ay + t * dy
    return ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5


def polygon_coverage(x: float, y: float, polys) -> float:
    """How much of this pixel the filled polygons cover, 0 to 1, with a soft edge."""
    best = 0.0
    for poly in polys:
        edge = min(
            distance_to_segment(x, y, *poly[i], *poly[(i + 1) % len(poly)])
            for i in range(len(poly))
        )
        signed = edge if point_in_polygon(x, y, poly) else -edge
        coverage = clamp01(0.5 + signed / FEATHER)
        if coverage > best:
            best = coverage
            if best >= 1.0:
                break
    return best


def corner_coverage(x: float, y: float) -> float:
    """How much of this pixel the rounded square covers, 0 to 1."""
    # Distance past the corner arc, for whichever corner this pixel is near.
    cx = CORNER if x < CORNER else (SIZE - CORNER - 1 if x > SIZE - CORNER - 1 else None)
    cy = CORNER if y < CORNER else (SIZE - CORNER - 1 if y > SIZE - CORNER - 1 else None)
    if cx is None or cy is None:
        return 1.0

    d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
    return clamp01((CORNER + FEATHER * 0.5 - d) / FEATHER)


def render() -> bytes:
    """Draws |⟩ and returns 8-bit RGBA scanlines, unfiltered."""
    k = SIZE / 1024
    polys = tuple(tuple((x * k, y * k) for x, y in poly) for poly in (BAR, BRACKET))

    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)  # filter type 0 (None)
        for x in range(SIZE):
            alpha = corner_coverage(x, y)
            if alpha <= 0.0:
                raw += bytes((0, 0, 0, 0))
                continue

            ink = polygon_coverage(x, y, polys)
            pixel = tuple(
                round(BG[i] + (INK[i] - BG[i]) * ink) for i in range(3)
            )
            raw += bytes((*pixel, round(alpha * 255)))

    return bytes(raw)


def png(raw: bytes, width: int = SIZE, height: int = SIZE, colour_type: int = 6) -> bytes:
    """Wraps 8-bit scanlines in a PNG container: colour type 6 is RGBA, 2 is RGB."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, colour_type, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


# The phone's assets. Each is the same two polygons, scaled and placed, and
# sampled at pixel centres so that a mark placed at the canvas centre is
# centred to the pixel.

# An Android adaptive icon layer is 108dp, of which the launcher shows the
# middle 72dp. Drawing the mark at 72/108 of its desktop size keeps it the same
# share of the visible tile as on the desktop icon, and well inside the 66dp
# safe circle.
ADAPTIVE_SCALE = 72 / 108

# Android's notification icon is a 24dp square with a 20dp live area; 96px is
# its xxxhdpi size. The mark's height fills the live area.
NOTIFICATION_SIZE = 96
NOTIFICATION_SCALE = NOTIFICATION_SIZE * 20 / 24 / (MARK_BOTTOM - MARK_TOP)

# The splash image is cropped to the mark, which spans its full width.
SPLASH_WIDTH = 632


def place(scale: float, ox: float, oy: float):
    """The mark's polygons in pixel space: scaled, then moved by (ox, oy)."""
    return tuple(
        tuple((x * scale + ox, y * scale + oy) for x, y in poly) for poly in (BAR, BRACKET)
    )


def centred(scale: float, size: int):
    """The mark scaled about the 1024 box's centre and put at a square canvas's centre."""
    offset = size / 2 - 512 * scale
    return place(scale, offset, offset)


def coverage(width: int, height: int, polys) -> list[float]:
    """How much of each pixel the mark covers, 0 to 1, row by row."""
    xs = [x for poly in polys for x, _ in poly]
    ys = [y for poly in polys for _, y in poly]
    left, right = min(xs) - FEATHER, max(xs) + FEATHER
    top, bottom = min(ys) - FEATHER, max(ys) + FEATHER

    cover = [0.0] * (width * height)
    for y in range(height):
        py = y + 0.5
        if py < top or py > bottom:
            continue
        for x in range(width):
            px = x + 0.5
            if left <= px <= right:
                cover[y * width + x] = polygon_coverage(px, py, polys)
    return cover


def ink_on(cover: list[float], width: int, bg, ink) -> bytes:
    """Opaque RGB scanlines: the ink laid over a solid background."""
    raw = bytearray()
    for i, c in enumerate(cover):
        if i % width == 0:
            raw.append(0)  # filter type 0 (None)
        raw += bytes(round(bg[j] + (ink[j] - bg[j]) * c) for j in range(3))
    return bytes(raw)


def ink_alone(cover: list[float], width: int, ink) -> bytes:
    """RGBA scanlines: the ink on transparent. Every pixel carries the ink's
    colour, even a clear one, so resampling never pulls black into the edge."""
    raw = bytearray()
    for i, c in enumerate(cover):
        if i % width == 0:
            raw.append(0)  # filter type 0 (None)
        raw += bytes((*ink, round(c * 255)))
    return bytes(raw)


def svg() -> str:
    """The mark as an SVG on the 1024 canvas, for Icon Composer."""

    def path(poly) -> str:
        return "M" + " L".join(f"{x:g} {y:g}" for x, y in poly) + " Z"

    colour = "#{:02x}{:02x}{:02x}".format(*INK)
    return (
        '<svg width="1024" height="1024" viewBox="0 0 1024 1024" fill="none" '
        'xmlns="http://www.w3.org/2000/svg">\n'
        f'<path d="{path(BAR)}" fill="{colour}"/>\n'
        f'<path d="{path(BRACKET)}" fill="{colour}"/>\n'
        "</svg>\n"
    )


def icon_json() -> str:
    """Icon Composer's document: a solid backdrop and one layer, the mark.

    Translucency is off so the mark reads as INK, not as INK mixed with the
    backdrop; the glass keeps its specular edge and neutral shadow.
    """
    fill = "srgb:" + ",".join(f"{c / 255:.5f}" for c in PHONE_BG) + ",1.00000"
    document = {
        "fill": {"solid": fill},
        "groups": [
            {
                "layers": [{"image-name": "ket.svg", "name": "ket"}],
                "shadow": {"kind": "neutral", "opacity": 0.5},
                "translucency": {"enabled": False, "value": 0.5},
            }
        ],
        "supported-platforms": {"circles": ["watchOS"], "squares": "shared"},
    }
    return json.dumps(document, indent=2, separators=(",", " : ")) + "\n"


def write(path: pathlib.Path, data: bytes | str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        path.write_text(data)
    else:
        path.write_bytes(data)
    print(f"wrote {path}")


def phone(root: pathlib.Path) -> int:
    """Writes the phone app's icons and splash into mobile/app/assets."""
    assets = root / "mobile" / "app" / "assets"
    images = assets / "images"

    # The fallback icon, and Android's legacy one: full bleed and opaque, as
    # the OS masks it. The mark sits as it does on the desktop icon.
    cover = coverage(SIZE, SIZE, centred(1.0, SIZE))
    write(images / "icon.png", png(ink_on(cover, SIZE, PHONE_BG, INK), colour_type=2))

    # Android's adaptive foreground, and its monochrome twin (only its alpha
    # is used). The background is app.json's backgroundColor.
    cover = coverage(SIZE, SIZE, centred(ADAPTIVE_SCALE, SIZE))
    write(images / "android-icon-foreground.png", png(ink_alone(cover, SIZE, INK)))
    write(images / "android-icon-monochrome.png", png(ink_alone(cover, SIZE, WHITE)))

    # The splash mark, cropped to its bounds; expo-splash-screen sizes it.
    scale = SPLASH_WIDTH / (MARK_RIGHT - MARK_LEFT)
    height = round((MARK_BOTTOM - MARK_TOP) * scale)
    polys = place(
        scale,
        -MARK_LEFT * scale,
        height / 2 - (MARK_TOP + MARK_BOTTOM) / 2 * scale,
    )
    cover = coverage(SPLASH_WIDTH, height, polys)
    write(images / "splash-icon.png", png(ink_alone(cover, SPLASH_WIDTH, INK), SPLASH_WIDTH, height))

    # Android's status-bar icon: white on transparent, as the system tints it.
    cover = coverage(NOTIFICATION_SIZE, NOTIFICATION_SIZE, centred(NOTIFICATION_SCALE, NOTIFICATION_SIZE))
    write(
        images / "notification-icon.png",
        png(ink_alone(cover, NOTIFICATION_SIZE, WHITE), NOTIFICATION_SIZE, NOTIFICATION_SIZE),
    )

    # iOS: an Icon Composer document, so the system can render it as glass.
    write(assets / "ket.icon" / "icon.json", icon_json())
    write(assets / "ket.icon" / "Assets" / "ket.svg", svg())
    return 0


def desktop(root: pathlib.Path) -> int:
    """Writes assets/ket.png and assets/ket.icns."""
    assets = root / "assets"
    assets.mkdir(exist_ok=True)

    source = assets / "ket.png"
    source.write_bytes(png(render()))
    print(f"wrote {source}")

    iconset = assets / "ket.iconset"
    if iconset.exists():
        for stale in iconset.iterdir():
            stale.unlink()
    else:
        iconset.mkdir()

    # The sizes iconutil expects, each with its @2x retina variant.
    for size in (16, 32, 128, 256, 512):
        for scale, suffix in ((1, ""), (2, "@2x")):
            pixels = size * scale
            subprocess.run(
                [
                    "sips", "-z", str(pixels), str(pixels), str(source),
                    "--out", str(iconset / f"icon_{size}x{size}{suffix}.png"),
                ],
                check=True,
                capture_output=True,
            )

    icns = assets / "ket.icns"
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(icns)], check=True)
    print(f"wrote {icns}")

    # The iconset is scaffolding; the .icns is what the bundle consumes.
    for built in iconset.iterdir():
        built.unlink()
    iconset.rmdir()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Draws ket's icon, the Dirac ket |⟩.")
    parser.add_argument(
        "--phone",
        action="store_true",
        help="write the phone app's icons and splash into mobile/app/assets instead",
    )
    args = parser.parse_args()

    root = pathlib.Path(__file__).resolve().parent.parent
    return phone(root) if args.phone else desktop(root)


if __name__ == "__main__":
    sys.exit(main())
