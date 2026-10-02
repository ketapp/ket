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
    python3 scripts/make-icon.py     # writes assets/ket.png and assets/ket.icns
"""

from __future__ import annotations

import pathlib
import struct
import subprocess
import sys
import zlib

SIZE = 1024

# Matches the theme in crates/ket-ui/src/main.rs, so the icon and the window agree.
BG = (0x1B, 0x1F, 0x23)
INK = (0x7E, 0xE0, 0x8A)

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
    # The mark is two filled shapes, drawn the way ⟩ sets in a maths face: a
    # bar and a bracket with a 60° apex, mitred to a sharp point, every end cut
    # flat. Diagonals read heavier than verticals at equal width, so the
    # bracket is a little thinner than the bar. The numbers are in a 1024 box
    # and are shared with the logo canvas; keep them in step.
    k = SIZE / 1024

    bar = (
        (301 * k, 272 * k), (401 * k, 272 * k),
        (401 * k, 752 * k), (301 * k, 752 * k),
    )
    bracket = (
        (437.8 * k, 272 * k), (544.3 * k, 272 * k), (722.3 * k, 512 * k),
        (544.3 * k, 752 * k), (437.8 * k, 752 * k), (539.7 * k, 512 * k),
    )
    polys = (bar, bracket)

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


def png(raw: bytes) -> bytes:
    """Wraps RGBA scanlines in a PNG container."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
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


if __name__ == "__main__":
    sys.exit(main())
