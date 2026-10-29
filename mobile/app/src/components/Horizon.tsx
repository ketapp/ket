// The planet under an empty Needs you tab: the desktop's own welcome picture
// (crates/ket-ui/src/horizon.rs), a sphere of glyphs rising from under the
// tab bar, lit at its rim, a grid turning slowly across it, stars above. The
// copy sits in its sky.
//
// A character field, one line of mono per row with a run per ink, as Ping
// draws its rings. The desktop redraws it twenty times a second; a phone has
// a battery, so this one ticks four times a second, and every step of its
// motion is sized to that tick so nothing reads as stuttering. A row whose
// glyphs did not change between ticks is not rendered again. It does not
// tick at all under Reduce Motion, while the screen is out of sight, or
// while the app is in the background.

import { useFocusEffect } from 'expo-router';
import { memo, useCallback, useEffect, useState } from 'react';
import { AppState, StyleSheet, Text, View, type LayoutChangeEvent } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';

import { font, t, tint } from '../lib/theme';

/** The glyphs' size and the rows' pitch. */
const GLYPH = 10;
const ROW = 12;

/** A cell's width: Geist Mono's advance is 0.6 of its size. */
const CELL = GLYPH * 0.6;

/** Where the planet's top sits, as a share of the field's height from the
 * top. The copy sits above it. */
export const HORIZON = 0.56;

/** The sphere's radius as a share of the field's width, and the least it
 * is: wide enough that its arc reads as a horizon rather than a ball. */
const RADIUS = 1.15;
const RADIUS_MIN = 400;

/** How far the glow reaches off the sphere's edge. */
const HALO = 60;

/** Frames a second while it moves. */
const FPS = 4;

/** One full turn of the grid, in seconds, and the turns the grid's lines
 * make in it. */
const CYCLE = 120;
const TURNS = 9;

/** How long the glyphs hold before they flicker, and the halo before it
 * shimmers: whole ticks, so every tick that changes them changes them
 * fully. */
const FLICKER = 0.5;
const SHIMMER = 0.25;

/** Glyphs, brightest first within each ink. */
const RIM = ['@', '#', '%', '*'];
const MID = ['*', '+', '=', 'o'];
const LOW = ['.', ':', '-', '~'];

/** A cell: its glyph and which ink — bright, mid, low — or a space. */
type Cell = [string, number] | null;

/** The planet, filling the view it is given, in `ink`. */
export function Horizon({ ink = t.text }: { ink?: string }) {
  const still = useReducedMotion();
  const [tick, setTick] = useState(0);
  const [area, setArea] = useState({ width: 0, height: 0 });
  const [focused, setFocused] = useState(false);
  const [active, setActive] = useState(AppState.currentState !== 'background');

  useFocusEffect(
    useCallback(() => {
      setFocused(true);
      return () => setFocused(false);
    }, []),
  );
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => setActive(state === 'active'));
    return () => sub.remove();
  }, []);
  useEffect(() => {
    if (still || !focused || !active) return;
    const timer = setInterval(() => setTick((n) => n + 1), 1000 / FPS);
    return () => clearInterval(timer);
  }, [still, focused, active]);

  const onLayout = useCallback((event: LayoutChangeEvent) => {
    const { width, height } = event.nativeEvent.layout;
    setArea((was) => (was.width === width && was.height === height ? was : { width, height }));
  }, []);

  const inks = [ink, tint(ink, 0.62), tint(ink, 0.3)];
  const rows = area.width > 0 ? field(area.width, area.height, still ? 7 : tick / FPS) : [];
  return (
    <View
      accessible
      accessibilityRole="image"
      accessibilityLabel="A planet of glyphs under a starry sky"
      style={styles.field}
      onLayout={onLayout}
      pointerEvents="none"
    >
      {rows.map((cells, row) => {
        const line = runs(cells);
        return <Row key={row} signature={line.map(([text, ink]) => `${ink}${text}`).join('\u001f')} line={line} inks={inks} />;
      })}
    </View>
  );
}

/** One row. Rendered again only when its glyphs or inks changed: a row of
 * sky holds still between the stars' twinkles, so most ticks redraw only
 * the planet. */
const Row = memo(
  function Row({ line, inks }: { signature: string; line: [string, number | null][]; inks: string[] }) {
    return (
      <Text style={styles.row} allowFontScaling={false} numberOfLines={1}>
        {line.map(([text, ink], index) => (
          <Text key={index} style={{ color: ink === null ? 'transparent' : inks[ink] }}>
            {text}
          </Text>
        ))}
      </Text>
    );
  },
  (before, after) => before.signature === after.signature && before.inks === after.inks,
);

/** One frame of the field, `seconds` into its motion. */
export function field(width: number, height: number, seconds: number): Cell[][] {
  const cols = Math.ceil(width / CELL);
  const rows = Math.ceil(height / ROW);
  const radius = Math.max(width * RADIUS, RADIUS_MIN);
  const centreX = width / 2;
  const centreY = height * HORIZON + radius;
  const turn = (seconds * TURNS * 2 * Math.PI) / CYCLE;
  const flicker = Math.floor(seconds / FLICKER);
  const shimmer = Math.floor(seconds / SHIMMER) + 11;
  const pick = (set: string[], roll: number) => set[Math.min(Math.floor(roll * 4), 3)];

  const grid: Cell[][] = [];
  for (let row = 0; row < rows; row++) {
    const cells: Cell[] = Array<Cell>(cols).fill(null);
    const y = row * ROW + ROW / 2 - centreY;
    for (let col = 0; col < cols; col++) {
      const x = col * CELL + CELL / 2 - centreX;
      const distance = Math.hypot(x, y);
      if (distance <= radius) {
        const nx = x / radius;
        const ny = y / radius;
        const nz = Math.sqrt(Math.max(1 - nx * nx - ny * ny, 0));
        // Bright where the surface turns away: the rim of a lit horizon.
        const rim = Math.pow(1 - nz, 2.4);
        const longitude = Math.atan2(nx, nz) * 14 + turn;
        const latitude = Math.asin(Math.max(-1, Math.min(1, ny))) * 30;
        const grid = Math.abs(Math.sin(longitude)) > 0.97 || Math.abs(Math.sin(latitude)) > 0.985 ? 0.42 : 0;
        const light = rim * 1.15 + grid * (0.35 + rim) + (nx < -0.1 ? 0.06 : 0);
        const roll = hash(col, row, flicker);
        if (light > 0.72) cells[col] = [pick(RIM, roll), 0];
        else if (light > 0.36) cells[col] = [pick(MID, roll), 1];
        else if (light > 0.1 || roll < 0.28) cells[col] = [pick(LOW, roll), 2];
      } else if (distance < radius + HALO) {
        const reach = 1 - (distance - radius) / HALO;
        if (hash(col, row, shimmer) < reach * reach * 0.55) {
          cells[col] = distance < radius + 16 ? ['=', 1] : ['.', 2];
        }
      } else if (hash(col, row, 5) < 0.012) {
        // Stars: fixed places, each twinkling on its own beat.
        const twinkle = hash(col, row, flicker);
        cells[col] = twinkle > 0.85 ? ['+', 1] : twinkle > 0.4 ? ['.', 2] : ['·', 2];
      }
    }
    grid.push(cells);
  }
  return grid;
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

/** A stable pseudo-random number in 0..1 for a cell at a step, the one the
 * desktop uses, so the field is steady between the steps of its flicker. */
function hash(x: number, y: number, step: number): number {
  let n = (Math.imul(x, 374_761_393) + Math.imul(y, 668_265_263) + Math.imul(step, 1_274_126_177)) | 0;
  n = Math.imul(n ^ (n >>> 13), 1_103_515_245);
  n ^= n >>> 16;
  return ((n >>> 0) % 10_000) / 10_000;
}

const styles = StyleSheet.create({
  field: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, overflow: 'hidden' },
  row: { fontFamily: font.chrome, fontSize: GLYPH, lineHeight: ROW, letterSpacing: 0, color: t.dim },
});
