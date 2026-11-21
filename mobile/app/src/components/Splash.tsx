// The launch splash, played. The native splash (expo-splash-screen, app.json)
// is a still: the mark on the ground, 64 pt tall and centred. The moment the
// app can draw, this draws that same frame over it, hides the native one
// underneath, and plays Collapse: the chevron fans out into four faint states
// and they snap back into one with a small flash, as a ket collapses to one
// outcome. It holds on the plain mark until the app is ready, then the mark
// recedes as the app rises in under it. Under Reduce Motion the frame only
// fades once the app is ready.

import * as SplashScreen from 'expo-splash-screen';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { StyleSheet, View } from 'react-native';
import Animated, {
  Easing,
  ReduceMotion,
  runOnJS,
  useAnimatedStyle,
  useReducedMotion,
  useSharedValue,
  withTiming,
  type SharedValue,
} from 'react-native-reanimated';
import Svg, { Polygon } from 'react-native-svg';

/** The native splash's ground and ink, fixed in app.json whatever the theme. */
const GROUND = '#0d0c0b';
const INK = '#7ee08a';
/** What the mark brightens toward as the states land. */
const FLASH = '#d4f7d8';

// The mark in scripts/make-icon.py's 1024 box, the numbers splash-icon.png is
// drawn from, so the first frame lands on the native one exactly.
const BAR = '301,272 401,272 401,752 301,752';
const BRACKET = '437.8,272 544.3,272 722.3,512 544.3,752 437.8,752 539.7,512';
const VIEW_BOX = '301 272 421.3 480';
const HEIGHT = 64;
const K = HEIGHT / 480;
const WIDTH = 421.3 * K;
/** The ghosts turn about the bar's inner edge, halfway down. */
const PIVOT = [(401 - 301) * K, (512 - 272) * K, 0];

/** Where the clock waits for the app, on the plain mark, and where it ends. */
const HOLD_MS = 440;
const END_MS = 600;
/** The Reduce Motion fade. */
const FADE_MS = 220;
const linear = { easing: Easing.linear, reduceMotion: ReduceMotion.Never };

/** The four faint states, outermost the faintest. */
const GHOSTS = [
  { angle: -20, weight: 0.2 },
  { angle: -10, weight: 0.34 },
  { angle: 10, weight: 0.34 },
  { angle: 20, weight: 0.2 },
];

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
/** How far the states are fanned: out by 240 ms, back in by 380. */
function spread(t: number) {
  'worklet';
  return outCubic(seg(t, 40, 240)) * (1 - seg(t, 240, 380) ** 3);
}
/** The flash as they land, 380 to 440 ms. */
function pulse(t: number) {
  'worklet';
  return Math.sin(Math.PI * seg(t, 380, 440));
}
/** The reveal, 440 to 600 ms. */
function reveal(t: number) {
  'worklet';
  return seg(t, HOLD_MS, END_MS);
}

function Shape({ points, fill }: { points: string[]; fill: string }) {
  return (
    <Svg width={WIDTH} height={HEIGHT} viewBox={VIEW_BOX}>
      {points.map((p) => (
        <Polygon key={p} points={p} fill={fill} />
      ))}
    </Svg>
  );
}

function Ghost({ t, angle, weight }: { t: SharedValue<number>; angle: number; weight: number }) {
  const style = useAnimatedStyle(() => {
    const s = spread(t.get());
    return { opacity: weight * Math.min(1, s * 4), transform: [{ rotate: `${angle * s}deg` }] };
  });
  return (
    <Animated.View style={[styles.layer, { transformOrigin: PIVOT }, style]}>
      <Shape points={[BRACKET]} fill={INK} />
    </Animated.View>
  );
}

/**
 * Draws the app, with the splash over it until it has played. `ready` says
 * the app has something to show; the splash holds on the plain mark until
 * then. The app rises 12 pt into place as the splash leaves.
 */
export function Splash({ ready, children }: { ready: boolean; children: ReactNode }) {
  const still = useReducedMotion();
  const t = useSharedValue(0);
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

  useEffect(() => {
    if (!ready || !held || leaving.current) return;
    leaving.current = true;
    const end = (finished?: boolean) => {
      'worklet';
      if (finished) runOnJS(setDone)(true);
    };
    if (still) fade.set(withTiming(0, { duration: FADE_MS, ...linear }, end));
    else t.set(withTiming(END_MS, { duration: END_MS - HOLD_MS, ...linear }, end));
  }, [ready, held, still, t, fade]);

  const rise = useAnimatedStyle(() => ({
    transform: [{ translateY: still ? 0 : 12 * (1 - outCubic(reveal(t.get()))) }],
  }));
  const cover = useAnimatedStyle(() => ({ opacity: fade.get() }));
  const ground = useAnimatedStyle(() => ({ opacity: 1 - outQuad(reveal(t.get())) }));
  const mark = useAnimatedStyle(() => {
    const now = t.get();
    return {
      opacity: 1 - outQuad(seg(now, HOLD_MS, 560)),
      transform: [{ scale: (1 + 0.05 * pulse(now)) * (1 - 0.1 * outCubic(reveal(now))) }],
    };
  });
  const chevron = useAnimatedStyle(() => ({ opacity: 1 - 0.45 * spread(t.get()) }));
  const flash = useAnimatedStyle(() => ({ opacity: 0.7 * pulse(t.get()) }));

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
            {GHOSTS.map((g) => (
              <Ghost key={g.angle} t={t} angle={g.angle} weight={g.weight} />
            ))}
            <View style={styles.layer}>
              <Shape points={[BAR]} fill={INK} />
            </View>
            <Animated.View style={[styles.layer, chevron]}>
              <Shape points={[BRACKET]} fill={INK} />
            </Animated.View>
            {/* A lighter copy over the mark: at opacity a it mixes the ink a of
                the way to FLASH. */}
            <Animated.View style={[styles.layer, flash]}>
              <Shape points={[BAR, BRACKET]} fill={FLASH} />
            </Animated.View>
          </Animated.View>
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
  layer: { ...StyleSheet.absoluteFill },
});
