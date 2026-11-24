// The launch splash, played. The native splash (expo-splash-screen, app.json)
// is a still: the mark on the ground, 64 pt tall and centred. The moment the
// app can draw, this draws that same frame over it, hides the native one
// underneath, and plays Collapse in glyphs: the mark reads out as noise and
// settles into a field of characters, shaded from the upper left as the
// Connecting planet is (Ping.tsx). Its chevron flickers and splits into four
// faint copies a row and two rows out, as a ket's states, and they snap back
// into one with a flash of `@`, as it collapses to one outcome. It holds on
// the glyph mark, shimmering, until the app is ready, then the glyphs run
// down the ramp to nothing as the app rises in under them. Under Reduce
// Motion the still frame only fades once the app is ready.
//
// The glyphs move on a 24 fps step. Every frame is worked out once and
// stacked, and the UI thread's clock shows one at a time, so a busy JS
// thread at launch can't stall it.

import * as SplashScreen from 'expo-splash-screen';
import { memo, useEffect, useRef, useState, type ReactNode } from 'react';
import { StyleSheet, Text } from 'react-native';
import Animated, {
  cancelAnimation,
  Easing,
  ReduceMotion,
  runOnJS,
  useAnimatedStyle,
  useReducedMotion,
  useSharedValue,
  withRepeat,
  withTiming,
  type SharedValue,
} from 'react-native-reanimated';
import Svg, { Polygon } from 'react-native-svg';

import { font, tint } from '../lib/theme';

/** The native splash's ground and ink, fixed in app.json whatever the theme. */
const GROUND = '#0d0c0b';
const INK = '#7ee08a';
/** The flash as the states land. */
const FLASH = '#d4f7d8';
const INK_SPLIT = tint(INK, 0.85);
const INK_MID = tint(INK, 0.7);
const INK_ECHO = tint(INK, 0.5);
const INK_LOW = tint(INK, 0.42);
const INK_FAINT = tint(INK, 0.34);

// The mark in scripts/make-icon.py's 1024 box, the numbers splash-icon.png is
// drawn from, so the first frame lands on the native one exactly.
const BAR = '301,272 401,272 401,752 301,752';
const BRACKET = '437.8,272 544.3,272 722.3,512 544.3,752 437.8,752 539.7,512';
const VIEW_BOX = '301 272 421.3 480';
const HEIGHT = 64;
const K = HEIGHT / 480;
const WIDTH = 421.3 * K;

/** The field over the mark: 7 pt glyphs on 8 pt rows, sixteen of them, so
 * rows break exactly on the mark's top and bottom edges. A cell is the code
 * face's advance, Fira Code's, 0.615 of its size. */
const GLYPH = 7;
const ROW = 8;
const CELL = GLYPH * 0.615;
const COLS = 31;
const ROWS = 16;
const FIELD_W = COLS * CELL;
const FIELD_H = ROWS * ROW;

/** The ramp, faintest first. */
const SHADE = ['.', ':', '-', '=', '+', '*', '#', '%', '@'];

/** One glyph step: 24 a second. */
const STEP = 1000 / 24;
/** The frames up to the hold, the hold's shimmer, and the reveal's. */
const RUN_FRAMES = 12;
const HOLD_FRAMES = 4;
const REVEAL_FRAMES = 4;
/** How long each shimmer frame of the hold lasts. */
const SHIMMER_MS = 250;
/** Where the clock waits for the app, on the glyph mark, and where it ends. */
const HOLD_MS = RUN_FRAMES * STEP;
const END_MS = 680;
/** The Reduce Motion fade. */
const FADE_MS = 220;
const linear = { easing: Easing.linear, reduceMotion: ReduceMotion.Never };

/** The four copies the chevron splits into: rows out, and their weight. */
const ECHOES = [
  [-2, 0.45],
  [-1, 0.8],
  [1, 0.8],
  [2, 0.45],
] as const;

function seg(t: number, from: number, to: number) {
  'worklet';
  return Math.min(1, Math.max(0, (t - from) / (to - from)));
}
function outCubic(x: number) {
  'worklet';
  return 1 - (1 - x) ** 3;
}
function outQuad(x: number) {
  'worklet';
  return 1 - (1 - x) ** 2;
}

/** Which stacked frame shows `now` ms in, `idle` ms into the hold. */
function frameAt(now: number, idle: number) {
  'worklet';
  if (now < HOLD_MS) return Math.min(RUN_FRAMES - 1, Math.floor(now / STEP));
  const step = Math.floor((now - HOLD_MS) / STEP);
  if (step === 0) return RUN_FRAMES + (Math.floor(idle / SHIMMER_MS) % HOLD_FRAMES);
  return RUN_FRAMES + HOLD_FRAMES + Math.min(step, REVEAL_FRAMES) - 1;
}

function clamp(x: number, from: number, to: number) {
  return Math.min(to, Math.max(from, x));
}
function inBar(x: number, y: number) {
  return x >= 301 && x <= 401 && y >= 272 && y <= 752;
}
function inBracket(x: number, y: number) {
  if (y < 272 || y > 752) return false;
  const d = Math.abs(y - 512) / 240;
  return x >= 539.7 - 101.9 * d && x <= 722.3 - 178 * d;
}
/** How much of a cell `inside` covers, from a 4 × 4 sample, read in the
 * 1024 box. */
function coverage(col: number, row: number, inside: (x: number, y: number) => boolean) {
  let hits = 0;
  for (let i = 0; i < 4; i++) {
    for (let j = 0; j < 4; j++) {
      const x = (col + (i + 0.5) / 4) * CELL - FIELD_W / 2;
      const y = (row + (j + 0.5) / 4) * ROW - FIELD_H / 2;
      if (inside(511.65 + x / K, 512 + y / K)) hits++;
    }
  }
  return hits / 16;
}

/** A cell of the field: what of the mark it covers, and how lit it is. */
type Spot = { col: number; row: number; bar: number; bracket: number; cover: number; light: number };

function spots(): Spot[] {
  const out: Spot[] = [];
  for (let row = 0; row < ROWS; row++) {
    for (let col = 0; col < COLS; col++) {
      const x = 511.65 + ((col + 0.5) * CELL - FIELD_W / 2) / K;
      const y = 512 + ((row + 0.5) * ROW - FIELD_H / 2) / K;
      const bar = coverage(col, row, inBar);
      const bracket = coverage(col, row, inBracket);
      // Lit from the upper left, as the Git view's planet is.
      const light = clamp(0.6 - 0.32 * ((x - 511.65) / 210) - 0.3 * ((y - 512) / 240), 0, 1);
      out.push({ col, row, bar, bracket, cover: Math.min(1, bar + bracket), light });
    }
  }
  return out;
}

/** A cell's resting glyph, 0 to 8 up the ramp, or -1 for none. */
function level(cover: number, light: number, jitter: number) {
  if (cover < 0.12) return -1;
  return clamp(Math.round(cover * (4.5 + 4 * clamp(light + jitter, 0, 1))), 1, 8);
}
function inkAt(k: number) {
  return k >= 6 ? INK : k >= 3 ? INK_MID : INK_LOW;
}

/** A cell: its glyph and ink, or a space. */
type Cell = [string, string] | null;

/** The field at glyph step `step`, its glyphs rolled with `shimmer`. */
function grid(cells: Spot[], step: number, shimmer: number): Cell[][] {
  const q = step * STEP;
  const spread = outCubic(seg(q, 120, 300)) * (1 - seg(q, 300, 410) ** 3);
  const flash = q >= 410 && q < HOLD_MS;
  const out: Cell[][] = Array.from({ length: ROWS }, () => Array<Cell>(COLS).fill(null));
  for (const s of cells) {
    let k = level(s.cover, s.light, (hash(s.col, s.row, shimmer) - 0.5) * 0.18);
    let ink = inkAt(k);
    // Turning: the solid shape reads out as noise first.
    if (k >= 0 && q < 120 && q < 50 + 70 * hash(s.col, s.row, 1)) {
      k = Math.floor(hash(s.col, s.row, step) * 9);
      ink = INK_MID;
    }
    if (spread > 0.05) {
      if (k >= 0 && s.bracket > s.bar) {
        // Superposed: the chevron's cells flicker up and down the ramp.
        k = clamp(k - 2 + Math.floor(hash(s.col, s.row, step) * 4 * spread), 1, 8);
        ink = INK_SPLIT;
      } else if (k < 0) {
        // And its copies, a row and two rows out.
        let weight = 0;
        for (const [rows, w] of ECHOES) {
          const dy = (rows * ROW * spread) / K;
          weight = Math.max(weight, coverage(s.col, s.row, (x, y) => inBracket(x, y - dy)) * w);
        }
        if (weight > 0.1) {
          const glyph = weight > 0.4 ? '=' : weight > 0.24 ? '-' : weight > 0.15 ? ':' : '.';
          out[s.row][s.col] = [glyph, weight > 0.24 ? INK_ECHO : INK_FAINT];
        }
        continue;
      }
    }
    if (k < 0) continue;
    if (flash && s.cover >= 0.3) {
      k = 8;
      ink = FLASH;
    }
    if (q > HOLD_MS) {
      // The reveal: each cell runs down the ramp, three glyphs a step, from
      // its own moment in the first 60 ms.
      const start = HOLD_MS + 60 * hash(s.col, s.row, 3);
      k -= Math.max(0, Math.floor(((q - start) / STEP) * 3));
      if (k < 0) continue;
      ink = inkAt(k);
    }
    out[s.row][s.col] = [SHADE[k], ink];
  }
  return out;
}

/** A run of one ink; a run of spaces has none. */
type Run = [string, string | null];

/** Each row as runs, spaces riding on the run before them, and the spaces at
 * its end left off. */
function lines(cells: Cell[][]): Run[][] {
  return cells.map((row) => {
    const out: Run[] = [];
    for (const cell of row) {
      const last = out[out.length - 1];
      const [glyph, ink] = cell ?? [' ', last ? last[1] : null];
      if (last && last[1] === ink) last[0] += glyph;
      else out.push([glyph, ink]);
    }
    while (out.length > 1 && out[out.length - 1][0].trim() === '') out.pop();
    return out;
  });
}

let built: Run[][][] | null = null;
/** Every frame, worked out the first time the splash plays: the run up to
 * the hold, the hold's shimmer, the reveal. */
function frames(): Run[][][] {
  if (built) return built;
  const cells = spots();
  const out: Run[][][] = [];
  for (let step = 0; step < RUN_FRAMES; step++) out.push(lines(grid(cells, step, Math.floor((step * STEP) / SHIMMER_MS))));
  for (let i = 0; i < HOLD_FRAMES; i++) out.push(lines(grid(cells, RUN_FRAMES, 100 + i)));
  for (let i = 1; i <= REVEAL_FRAMES; i++) out.push(lines(grid(cells, RUN_FRAMES + i, 100)));
  built = out;
  return out;
}

/** A stable pseudo-random number in 0..1 for a cell at a step, the one ket's
 * desktop uses. */
function hash(x: number, y: number, step: number): number {
  let n = (Math.imul(x, 374_761_393) + Math.imul(y, 668_265_263) + Math.imul(step, 1_274_126_177)) | 0;
  n = Math.imul(n ^ (n >>> 13), 1_103_515_245);
  n ^= n >>> 16;
  return ((n >>> 0) % 10_000) / 10_000;
}

/** One stacked frame, shown while the clock is on it. */
function Frame({ index, rows, t, idle }: { index: number; rows: Run[][]; t: SharedValue<number>; idle: SharedValue<number> }) {
  const shown = useAnimatedStyle(() => ({ opacity: frameAt(t.get(), idle.get()) === index ? 1 : 0 }));
  return (
    <Animated.Text style={[styles.glyphs, shown]} allowFontScaling={false}>
      {rows.map((runs, row) =>
        runs.map(([text, ink], i) => {
          const value = i === runs.length - 1 && row < rows.length - 1 ? `${text}\n` : text;
          return ink ? (
            <Text key={`${row}.${i}`} style={{ color: ink }}>
              {value}
            </Text>
          ) : (
            value
          );
        }),
      )}
    </Animated.Text>
  );
}

/** The glyph field, every frame stacked. Drawn once: the frames never
 * change, only which one shows. */
const Glyphs = memo(function Glyphs({ t, idle }: { t: SharedValue<number>; idle: SharedValue<number> }) {
  const all = frames();
  const field = useAnimatedStyle(() => {
    const now = t.get();
    return {
      opacity: seg(now, 40, 90) * (1 - seg(now, 610, END_MS)),
      transform: [{ scale: 1 - 0.08 * outCubic(seg(now, HOLD_MS, END_MS)) }],
    };
  });
  return (
    <Animated.View style={[styles.field, field]} pointerEvents="none">
      {all.map((rows, index) => (
        <Frame key={index} index={index} rows={rows} t={t} idle={idle} />
      ))}
    </Animated.View>
  );
});

function Shape({ points, fill }: { points: string[]; fill: string }) {
  return (
    <Svg width={WIDTH} height={HEIGHT} viewBox={VIEW_BOX}>
      {points.map((p) => (
        <Polygon key={p} points={p} fill={fill} />
      ))}
    </Svg>
  );
}

/**
 * Draws the app, with the splash over it until it has played. `ready` says
 * the app has something to show; the splash holds on the glyph mark until
 * then. The app rises 12 pt into place as the splash leaves.
 */
export function Splash({ ready, children }: { ready: boolean; children: ReactNode }) {
  const still = useReducedMotion();
  const t = useSharedValue(0);
  const idle = useSharedValue(0);
  const fade = useSharedValue(1);
  const [held, setHeld] = useState(false);
  const [done, setDone] = useState(false);
  const started = useRef(false);
  const leaving = useRef(false);

  // On the first frame drawn, the native splash goes, unseen, and the clock
  // runs to the hold.
  const start = () => {
    if (started.current) return;
    started.current = true;
    SplashScreen.hide();
    if (still) {
      setHeld(true);
      return;
    }
    t.set(
      withTiming(HOLD_MS, { duration: HOLD_MS, ...linear }, (finished) => {
        if (finished) runOnJS(setHeld)(true);
      }),
    );
  };

  // Held, the glyphs shimmer until the app is ready.
  useEffect(() => {
    if (!held || still || leaving.current) return;
    const cycle = SHIMMER_MS * HOLD_FRAMES;
    idle.set(withRepeat(withTiming(cycle, { duration: cycle, ...linear }), -1));
  }, [held, still, idle]);

  useEffect(() => {
    if (!ready || !held || leaving.current) return;
    leaving.current = true;
    cancelAnimation(idle);
    const end = (finished?: boolean) => {
      'worklet';
      if (finished) runOnJS(setDone)(true);
    };
    if (still) fade.set(withTiming(0, { duration: FADE_MS, ...linear }, end));
    else t.set(withTiming(END_MS, { duration: END_MS - HOLD_MS, ...linear }, end));
  }, [ready, held, still, t, fade, idle]);

  const rise = useAnimatedStyle(() => ({
    transform: [{ translateY: still ? 0 : 12 * (1 - outCubic(seg(t.get(), 520, END_MS))) }],
  }));
  const cover = useAnimatedStyle(() => ({ opacity: fade.get() }));
  const ground = useAnimatedStyle(() => ({ opacity: 1 - outQuad(seg(t.get(), 520, END_MS)) }));
  // The solid mark gives way to its glyphs.
  const mark = useAnimatedStyle(() => ({ opacity: 1 - seg(t.get(), 40, 110) }));

  return (
    <>
      <Animated.View style={[styles.fill, rise]}>{children}</Animated.View>
      {done ? null : (
        <Animated.View
          style={[styles.cover, cover]}
          onLayout={start}
          accessibilityElementsHidden
          importantForAccessibility="no-hide-descendants"
        >
          <Animated.View style={[styles.ground, ground]} />
          <Animated.View style={[styles.mark, mark]}>
            <Shape points={[BAR, BRACKET]} fill={INK} />
          </Animated.View>
          {still ? null : <Glyphs t={t} idle={idle} />}
        </Animated.View>
      )}
    </>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  // Over everything, the lock's cover included.
  cover: { ...StyleSheet.absoluteFill, zIndex: 200, alignItems: 'center', justifyContent: 'center' },
  ground: { ...StyleSheet.absoluteFill, backgroundColor: GROUND },
  mark: { width: WIDTH, height: HEIGHT },
  // Centred on the mark, as the mark is on the screen.
  field: {
    position: 'absolute',
    left: '50%',
    top: '50%',
    width: FIELD_W,
    height: FIELD_H,
    marginLeft: -FIELD_W / 2,
    marginTop: -FIELD_H / 2,
  },
  // A little wider than the field, so no row wraps on rounding.
  glyphs: {
    position: 'absolute',
    left: 0,
    top: 0,
    width: FIELD_W + 8,
    fontFamily: font.code,
    fontSize: GLYPH,
    lineHeight: ROW,
    letterSpacing: 0,
    includeFontPadding: false,
    color: INK,
  },
});
