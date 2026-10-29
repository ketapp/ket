// How a row's words change when a status update lands. The agent's line is
// wiped: erased from its right end, then drawn back in from the left with
// the new text, as if a cursor passed over it. The state word at the end of
// the row only fades out and in. Both run off one value, so the word is gone
// while the line is, and back as it is. Under Reduce Motion neither moves:
// the new text is simply there.

import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { StyleSheet, View, type StyleProp, type ViewStyle } from 'react-native';
import Animated, {
  Easing,
  ReduceMotion,
  runOnJS,
  useAnimatedStyle,
  useReducedMotion,
  useSharedValue,
  withTiming,
} from 'react-native-reanimated';

/** Each half of a swap: the old text going, the new text coming. */
const HALF_MS = 200;
/** Going: gathers speed, so the erase ends sharply at the left. */
const GO = Easing.bezier(0.5, 0, 1, 1);
/** Coming: fast off the mark, settling at the right end. */
const COME = Easing.bezier(0, 0, 0.3, 1);
const motion = { reduceMotion: ReduceMotion.System };

/** What a swap draws from: the words as data, and a token naming them. */
interface SwapProps<T> {
  /** Names what `item` says; the swap runs when it changes. */
  token: string;
  item: T;
  style?: StyleProp<ViewStyle>;
  /** Draws an item: the one on screen while it is erased, the newest after. */
  children: (item: T) => ReactNode;
}

/**
 * Runs the swap. While `token` matches what is on screen the newest item is
 * drawn. When it changes, the item last drawn stays up as `gone` runs 0 → 1,
 * the newest item takes its place, and `gone` runs back 1 → 0. A token that
 * changes again mid-swap is taken up when this swap lands.
 */
function useSwap<T>(token: string, item: T, still: boolean) {
  const gone = useSharedValue(0);
  // What is on screen, and so what the erase runs over.
  const [drawn, setDrawn] = useState({ token, item });
  // The newest, for the swap to land on.
  const latest = useRef({ token, item });
  const busy = useRef(false);
  const [, poke] = useState(0);

  // Still, nothing is held back: what is drawn is simply the newest.
  if (still && drawn.token !== token) setDrawn({ token, item });

  useEffect(() => {
    latest.current = { token, item };
  });

  const land = useCallback(() => {
    busy.current = false;
    poke((n) => n + 1);
  }, []);
  const arrive = useCallback(() => {
    setDrawn(latest.current);
    gone.set(
      withTiming(0, { duration: HALF_MS, easing: COME, ...motion }, (done) => {
        if (done) runOnJS(land)();
      }),
    );
  }, [gone, land]);

  useEffect(() => {
    if (still) {
      // Including a swap begun before Reduce Motion came on: it lands at once.
      gone.set(0);
      busy.current = false;
      return;
    }
    if (busy.current || token === drawn.token) return;
    busy.current = true;
    gone.set(
      withTiming(1, { duration: HALF_MS, easing: GO, ...motion }, (done) => {
        if (done) runOnJS(arrive)();
      }),
    );
  }, [still, token, drawn.token, gone, arrive]);

  const live = still || token === drawn.token;
  return { gone, shown: live ? item : drawn.item };
}

/**
 * The wipe. The line sits in a box that clips it; a cover in `fill`, the
 * colour behind it, grows in from the right edge over the old text and
 * shrinks back to the right edge off the new.
 */
export function Wipe<T>({ token, item, fill, style, children }: SwapProps<T> & { fill: string }) {
  const still = useReducedMotion();
  const { gone, shown } = useSwap(token, item, still);
  const cover = useAnimatedStyle(() => ({ transform: [{ scaleX: gone.get() }] }));
  if (still) {
    return <View style={[styles.line, style]}>{children(shown)}</View>;
  }
  return (
    <View style={[styles.line, style]}>
      {children(shown)}
      <Animated.View pointerEvents="none" style={[styles.cover, { backgroundColor: fill }, cover]} />
    </View>
  );
}

/** The fade: the words go out as the line is erased, and come back as it
 * is drawn. For the state word at the end of a row. */
export function Fade<T>({ token, item, style, children }: SwapProps<T>) {
  const still = useReducedMotion();
  const { gone, shown } = useSwap(token, item, still);
  const fade = useAnimatedStyle(() => ({ opacity: 1 - gone.get() }));
  if (still) {
    return <View style={style}>{children(shown)}</View>;
  }
  return <Animated.View style={[style, fade]}>{children(shown)}</Animated.View>;
}

const styles = StyleSheet.create({
  line: { flexDirection: 'row', alignItems: 'center', minWidth: 0, overflow: 'hidden' },
  // Hangs a hair past the text's bounds so no edge shows through, and grows
  // from its right edge, so it is full across however wide the new text is.
  cover: { position: 'absolute', top: -1, bottom: -1, left: -1, right: -1, transformOrigin: 'right' },
});
