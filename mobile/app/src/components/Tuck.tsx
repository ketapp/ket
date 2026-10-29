// How a card waiting on you leaves the conversation: it tucks away under the
// composer. The card floats over the conversation's foot, hung from the
// composer's top edge, and the room it takes there is handed to the scroll
// view, which keeps it clear; as the card slides down out of sight that room
// shrinks with it, so the conversation comes down after it frame for frame.
// A card answered here goes a moment after the tap, once the button tapped
// wears its mark. One the desktop would not take comes back up the way it
// went. Under Reduce Motion a card is simply gone.

import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { type LayoutChangeEvent, StyleSheet, type StyleProp, View, type ViewStyle } from 'react-native';
import Animated, {
  Easing,
  ReduceMotion,
  runOnJS,
  type SharedValue,
  useAnimatedReaction,
  useAnimatedStyle,
  useReducedMotion,
  useSharedValue,
  withDelay,
  withTiming,
} from 'react-native-reanimated';

/** The slide down under the composer, and back up. */
const TUCK_MS = 360;
/** How long the tapped button's mark shows before the card goes. */
const MARK_MS = 150;
/** Off quickly, settling as it lands: iOS's own sheet curve. */
const SLIDE = Easing.bezier(0.32, 0.72, 0, 1);
/** The card thins as it goes, mostly at the end, to this. */
const THIN = Easing.bezier(0.55, 0, 1, 0.45);
const THINNED = 0.4;
/** What a card says once it is answered: stepped back to this, this fast. */
const DIMMED = 0.45;
const DIM_MS = 140;
/** No bound: the card at its own height. */
const UNBOUND = 100_000;
const motion = { reduceMotion: ReduceMotion.System };

/** What a card is drawn from. A card under a new key is a new card. */
export type Tuckable = {
  key: string;
  /** Answered here, its tapped button wearing its mark: it goes, a moment
   * after, though the desktop still asks it. */
  marked?: boolean;
};

/** A card on its way in, there, or on its way out, for `Tucked` to draw. */
export type Tuck<T extends Tuckable> = {
  /** The card on screen: the one asked for, or the last while it goes. */
  card: T | undefined;
  going: boolean;
  style: ReturnType<typeof useAnimatedStyle>;
  measure: (e: LayoutChangeEvent) => void;
};

/**
 * Keeps the card asked for on screen, and the last one while it tucks away.
 * A new card asked for meanwhile is drawn once the last has gone. `room` is
 * the height the card takes over the conversation, for the scroll view's
 * `extraContentPadding`.
 */
export function useTuck<T extends Tuckable>(card: T | undefined, room: SharedValue<number>): Tuck<T> {
  const still = useReducedMotion();
  // 1 drawn in full, 0 tucked away.
  const open = useSharedValue(1);
  const strength = useSharedValue(1);
  // The card is bound to `open` of its height only while it goes or comes back.
  const moving = useSharedValue(false);
  // Its own height as last laid out, margin and all.
  const full = useSharedValue(0);
  // The card asked for, as first drawn under its key: what tucks away if
  // it stops being asked without a word. Kept by key, not by identity, as
  // the card is made afresh on every draw.
  const [seen, setSeen] = useState(card && !card.marked ? card : undefined);
  // The card on its way out, as it was when it went.
  const [leaving, setLeaving] = useState<T>();

  const asked = card && !card.marked ? card : undefined;
  if (asked && leaving?.key === asked.key) {
    // Back: the desktop would not take the answer.
    setLeaving(undefined);
  }
  if (asked && asked.key !== seen?.key) {
    setSeen(asked);
  } else if (!asked && seen) {
    setSeen(undefined);
    // Going with its mark, if it is still asked; as first drawn if not. One
    // asked and gone while another went was never drawn: it just goes.
    if (!leaving && !still) setLeaving(card?.key === seen.key ? card : seen);
  }

  // On screen: the card going until it has gone, then the one asked for.
  const drawn = leaving ?? asked;
  const key = drawn?.key;
  const going = leaving !== undefined;
  const marked = leaving?.marked ?? false;
  // The card that began to go, so one coming back is told from a new one.
  const left = useRef<string | undefined>(undefined);
  const gone = useCallback(() => setLeaving(undefined), []);

  useEffect(() => {
    if (key === undefined) {
      full.set(0);
      left.current = undefined;
      return;
    }
    if (going) {
      if (left.current === key) return;
      left.current = key;
      moving.set(true);
      const wait = marked ? MARK_MS : 0;
      open.set(
        withDelay(
          wait,
          withTiming(0, { duration: TUCK_MS, easing: SLIDE, ...motion }, (done) => {
            if (done) runOnJS(gone)();
          }),
        ),
      );
      strength.set(withDelay(wait, withTiming(THINNED, { duration: TUCK_MS, easing: THIN, ...motion })));
      return;
    }
    if (left.current === key) {
      open.set(
        withTiming(1, { duration: TUCK_MS, easing: SLIDE, ...motion }, (done) => {
          if (done) moving.set(false);
        }),
      );
      strength.set(withTiming(1, { duration: TUCK_MS, easing: SLIDE, ...motion }));
    } else {
      // A new card: there at once.
      moving.set(false);
      open.set(1);
      strength.set(1);
    }
    left.current = undefined;
  }, [key, going, marked, full, moving, open, strength, gone]);

  useAnimatedReaction(
    () => (moving.get() ? full.get() * open.get() : full.get()),
    (now, before) => {
      if (now !== before) room.set(now);
    },
  );

  const style = useAnimatedStyle(() =>
    moving.get() ? { maxHeight: full.get() * open.get(), opacity: strength.get() } : { maxHeight: UNBOUND, opacity: 1 },
  );
  const measure = useCallback((e: LayoutChangeEvent) => full.set(e.nativeEvent.layout.height), [full]);

  return { card: drawn, going, style, measure };
}

/** Draws the card on screen, cut off at the composer's top edge as it goes.
 * Hung under the composer's top edge, so it slides down behind it. */
export function Tucked({ tuck, children }: { tuck: Tuck<Tuckable>; children: ReactNode }) {
  return (
    <Animated.View pointerEvents={tuck.going ? 'none' : 'box-none'} style={[styles.clip, tuck.style]}>
      <View onLayout={tuck.measure}>{children}</View>
    </Animated.View>
  );
}

/** What a card says, stepping back once it is answered. */
export function Dim({ on, style, children }: { on: boolean; style?: StyleProp<ViewStyle>; children: ReactNode }) {
  const level = useSharedValue(on ? DIMMED : 1);
  useEffect(() => {
    level.set(withTiming(on ? DIMMED : 1, { duration: DIM_MS, ...motion }));
  }, [on, level]);
  const dimmed = useAnimatedStyle(() => ({ opacity: level.get() }));
  return <Animated.View style={[style, dimmed]}>{children}</Animated.View>;
}

const styles = StyleSheet.create({
  clip: { overflow: 'hidden' },
});
