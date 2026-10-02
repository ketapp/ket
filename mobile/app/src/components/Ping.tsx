// The picture while the phone reaches for its desktop: the desktop as a
// planet, drawn in glyphs the way ket's own Git view draws its orbit, with
// signal rings leaving it toward the phone — a green `o` out on the last ring.
// Stuck, the rings stop: one broken ring, a dimmed planet, and an `x` where
// the phone was.
//
// A character field, one line of mono per row with a run per ink, so it reads
// as part of ket rather than an illustration. The rings move while connecting
// (not under Reduce Motion, and not while the screen is out of sight).

import { useFocusEffect } from 'expo-router';
import { useCallback, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';

import { font, t, tint } from '../lib/theme';

/** The glyphs' size and the rows' pitch. */
const GLYPH = 11;
const ROW = 13;

/** A cell is about half as wide as it is tall: Geist Mono's advance is 0.6 of
 * its size. Circles are drawn against the real ratio so they stay round. */
const ASPECT = ROW / (GLYPH * 0.6);

/** The planet's surface, darkest first. */
const SHADE = ['.', ':', '-', '=', '+', '*', '#', '%', '@'];

/** Where the phone sits: out on the last ring, up and to the right. */
const PHONE_ANGLE = (-38 * Math.PI) / 180;
const PHONE_RING = 8.6;

/** The rings: where they start, how far they travel, how far apart, and how
 * fast, in rows and rows a second. */
const RING_FROM = 3.8;
const RING_SPAN = 6.0;
const RING_GAP = 3.0;
const RING_SPEED = 2.5;

/** Frames a second while the rings move. */
const FPS = 10;

/** A cell: its glyph and which ink — faint, dim, soft, text, accent. */
type Cell = [string, number] | null;

export function Ping({ stuck }: { stuck: boolean }) {
  const still = useReducedMotion();
  const [tick, setTick] = useState(0);

  useFocusEffect(
    useCallback(() => {
      if (stuck || still) return;
      const timer = setInterval(() => setTick((n) => n + 1), 1000 / FPS);
      return () => clearInterval(timer);
    }, [stuck, still]),
  );

  const rows = stuck ? field(true, 0, 40, 15) : field(false, still ? null : tick / FPS, 48, 21);
  const inks = [tint(t.dim, 0.3), tint(t.dim, 0.65), t.dim, t.text, stuck ? t.quotaWarm : t.running];
  return (
    <View
      accessible
      accessibilityRole="image"
      accessibilityLabel={stuck ? 'No signal from your desktop' : 'Signal rings leaving your desktop'}
      style={styles.field}
    >
      {rows.map((cells, row) => (
        <Text key={row} style={styles.row} allowFontScaling={false} numberOfLines={1}>
          {runs(cells).map(([text, ink], index) => (
            <Text key={index} style={{ color: ink === null ? 'transparent' : inks[ink] }}>
              {text}
            </Text>
          ))}
        </Text>
      ))}
    </View>
  );
}

/** One frame. `seconds` moves the rings; `null` is the still frame Reduce
 * Motion shows. */
function field(stuck: boolean, seconds: number | null, cols: number, rows: number): Cell[][] {
  const grid: Cell[][] = Array.from({ length: rows }, () => Array<Cell>(cols).fill(null));
  const cx = (cols - 1) / 2;
  const cy = (rows - 1) / 2;
  const put = (x: number, y: number, glyph: string, ink: number) => {
    const col = Math.round(x);
    const row = Math.round(y);
    if (row >= 0 && row < rows && col >= 0 && col < cols) grid[row][col] = [glyph, ink];
  };
  const ring = (radius: number, glyph: string, ink: number, broken = false) => {
    for (let step = 0; step < 360; step += 2) {
      if (broken && Math.floor(step / 18) % 2) continue;
      const th = (step * Math.PI) / 180;
      put(cx + radius * ASPECT * Math.cos(th), cy + radius * Math.sin(th), glyph, ink);
    }
  };

  for (let row = 0; row < rows; row++) {
    for (let col = 0; col < cols; col++) {
      if (hash(col, row, 5) < 0.03) grid[row][col] = ['.', 0];
    }
  }

  if (stuck) {
    ring(5.6, '.', 1, true);
    planet(grid, cx, cy, 3.0, true);
    put(cx + 6.6 * ASPECT * Math.cos(PHONE_ANGLE), cy + 6.6 * Math.sin(PHONE_ANGLE), 'x', 4);
    return grid;
  }

  if (seconds === null) {
    ring(5.4, '~', 4);
    ring(PHONE_RING, ':', 1);
  } else {
    // Rings leave the planet, green while new, fading as they spread.
    for (let k = 0; k * RING_GAP < RING_SPAN; k++) {
      const radius = RING_FROM + ((seconds * RING_SPEED + k * RING_GAP) % RING_SPAN);
      if (radius < 6.2) ring(radius, '~', 4);
      else if (radius < 8.0) ring(radius, ':', 2);
      else ring(radius, '.', 1);
    }
  }
  planet(grid, cx, cy, 3.4, false);
  put(cx + PHONE_RING * ASPECT * Math.cos(PHONE_ANGLE), cy + PHONE_RING * Math.sin(PHONE_ANGLE), 'o', 4);
  return grid;
}

/** A sphere of glyphs lit from the upper left, as the Git view's orbit draws
 * its planet. Dimmed, its brightest glyphs are left out. */
function planet(grid: Cell[][], cx: number, cy: number, radius: number, dim: boolean) {
  grid.forEach((cells, row) => {
    cells.forEach((_, col) => {
      const dx = (col - cx) / (radius * ASPECT);
      const dy = (row - cy) / radius;
      const d = Math.hypot(dx, dy);
      if (d >= 1) return;
      const light = Math.max(-dx * 0.8 - dy * 0.35 + Math.sqrt(1 - d * d) * 0.6, 0);
      let k = Math.min(Math.floor(light * SHADE.length), SHADE.length - 1);
      if (dim) {
        k = Math.min(k, 4);
        cells[col] = [SHADE[k], k < 3 ? 1 : 2];
      } else {
        cells[col] = [SHADE[k], k > 5 ? 3 : 2];
      }
    });
  });
}

/** A row as runs of one ink, spaces riding on the run before them. */
function runs(cells: Cell[]): [string, number | null][] {
  const out: [string, number | null][] = [];
  for (const cell of cells) {
    const last = out[out.length - 1];
    const [glyph, ink] = cell ?? [' ', last ? last[1] : null];
    if (last && last[1] === ink) last[0] += glyph;
    else out.push([glyph, ink]);
  }
  return out;
}

/** A stable pseudo-random number in 0..1 for a cell, the one ket's desktop
 * uses, so the stars sit where its stars sit. */
function hash(x: number, y: number, seed: number): number {
  let n = (Math.imul(x, 374_761_393) + Math.imul(y, 668_265_263) + Math.imul(seed, 1_274_126_177)) | 0;
  n = Math.imul(n ^ (n >>> 13), 1_103_515_245);
  n ^= n >>> 16;
  return ((n >>> 0) % 10_000) / 10_000;
}

const styles = StyleSheet.create({
  field: { alignItems: 'center' },
  row: { fontFamily: font.chrome, fontSize: GLYPH, lineHeight: ROW, letterSpacing: 0, color: t.dim },
});
