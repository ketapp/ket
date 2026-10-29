// The composer's moving parts: Send, which lights up and launches, and turns
// into Stop while the agent works; +, which turns into × as its menu opens;
// and the float the + and / menus open in, over the chat rather than
// pushing it. Every duration here is a Reanimated call, and every one
// follows Reduce Motion: with it on, each lands at its end at once.
//
// Nothing on the bar changes size or place but these: the chat above, the
// chips and the field stay where they are while a menu is up, so a menu
// can open with the keyboard up and the keyboard stays.

import { createContext, type ReactNode, useContext, useEffect, useState } from 'react';
import { Pressable, StyleSheet, type StyleProp, type ViewStyle } from 'react-native';
import Animated, {
  Easing,
  ReduceMotion,
  interpolateColor,
  useAnimatedStyle,
  useSharedValue,
  withDelay,
  withSequence,
  withSpring,
  withTiming,
} from 'react-native-reanimated';

import { size, t } from '../lib/theme';
import { Plus, Send, Stop } from './icons';

/** The one curve: out fast, settling slow. */
const OUT = Easing.bezier(0.2, 0.8, 0.2, 1);
/** Out with a little past the mark, for what opens or trades places. */
const OVER = Easing.bezier(0.34, 1.35, 0.64, 1);
const motion = { reduceMotion: ReduceMotion.System };
const timing = (ms: number, easing = OUT) => ({ duration: ms, easing, ...motion });
/** A press: down to nine tenths under the finger, sprung back on release. */
const PRESS = { damping: 18, stiffness: 320, ...motion };

/** Send, or Stop while the agent works with nothing to send: the same
 * place, the same size, so the thumb never moves.
 *
 * Ready — words in the field, or a file — it fills brand green with the
 * arrow in ink; otherwise grey with a faint arrow. `launches` counts
 * sends: each one shoots the arrow up and out, brings a fresh one up from
 * below, and flashes the fill. Send and Stop trade with a turn: the arrow
 * spins away to the right as the square spins in from the left. */
export function SendButton({
  ready,
  stop,
  stopping,
  launches,
  onPress,
}: {
  ready: boolean;
  /** Working, with nothing to send: the button stops the turn. */
  stop: boolean;
  /** Stop pressed and not yet taken: dimmed, and not pressable. */
  stopping: boolean;
  /** How many sends so far; each change launches the arrow. */
  launches: number;
  onPress: () => void;
}) {
  const lit = useSharedValue(ready ? 1 : 0);
  const stopped = useSharedValue(stop ? 1 : 0);
  const pressed = useSharedValue(1);
  const punch = useSharedValue(1);
  const flight = useSharedValue(0);
  const flash = useSharedValue(0);
  useEffect(() => {
    lit.set(withTiming(ready ? 1 : 0, timing(180)));
  }, [ready, lit]);
  useEffect(() => {
    stopped.set(withTiming(stop ? 1 : 0, timing(320, OVER)));
    // A pop, each way.
    punch.set(withSequence(withTiming(1.12, timing(140, OVER)), withTiming(1, timing(220, OUT))));
  }, [stop, stopped, punch]);
  useEffect(() => {
    if (launches === 0) return;
    flight.set(
      withSequence(
        withTiming(-34, { duration: 150, easing: Easing.in(Easing.quad), ...motion }),
        withTiming(34, { duration: 0 }),
        withTiming(0, timing(210)),
      ),
    );
    punch.set(withSequence(withTiming(0.9, timing(105)), withTiming(1.08, timing(150, OVER)), withTiming(1, timing(165))));
    flash.set(withSequence(withTiming(1, timing(125)), withTiming(0, timing(295))));
  }, [launches, flight, punch, flash]);

  const shell = useAnimatedStyle(() => ({
    backgroundColor: interpolateColor(
      flash.get(),
      [0, 1],
      [interpolateColor(lit.get(), [0, 1], [t.selection, t.brand]), '#d9ffe0'],
    ),
    transform: [{ scale: pressed.get() * punch.get() }],
    opacity: stopping ? 0.6 : 1,
  }));
  const arrow = useAnimatedStyle(() => {
    const s = stopped.get();
    return {
      opacity: 1 - s,
      transform: [{ translateY: flight.get() }, { rotate: `${120 * s}deg` }, { scale: 1 - 0.7 * s }],
    };
  });
  const square = useAnimatedStyle(() => {
    const s = stopped.get();
    return { opacity: s, transform: [{ rotate: `${-120 * (1 - s)}deg` }, { scale: 0.3 + 0.7 * s }] };
  });
  return (
    <Animated.View style={[styles.round, styles.send, shell]}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={stop ? 'Stop' : 'Send'}
        disabled={stop ? stopping : !ready}
        onPress={onPress}
        onPressIn={() => pressed.set(withSpring(0.9, PRESS))}
        onPressOut={() => pressed.set(withSpring(1, PRESS))}
        style={styles.fill}
      >
        <Animated.View style={[styles.glyph, arrow]}>
          <Glyph ready={ready} />
        </Animated.View>
        <Animated.View style={[styles.glyph, square]}>
          <Stop size={18} color={t.text} />
        </Animated.View>
      </Pressable>
    </Animated.View>
  );
}

/** The arrow, in ink on the green and faint on the grey. Two drawn and
 * cross-faded: an SVG stroke cannot be driven by a shared value. */
function Glyph({ ready }: { ready: boolean }) {
  const ink = useSharedValue(ready ? 1 : 0);
  useEffect(() => {
    ink.set(withTiming(ready ? 1 : 0, timing(180)));
  }, [ready, ink]);
  const dark = useAnimatedStyle(() => ({ opacity: ink.get() }));
  const faint = useAnimatedStyle(() => ({ opacity: 1 - ink.get() }));
  return (
    <>
      <Animated.View style={[StyleSheet.absoluteFill, styles.center, faint]}>
        <Send size={18} color={t.faint} weight={2.3} />
      </Animated.View>
      <Animated.View style={[styles.center, dark]}>
        <Send size={18} color={t.onBrand} weight={2.3} />
      </Animated.View>
    </>
  );
}

/** +, which turns a quarter to become × as its menu opens, fills with
 * selection and pops once; back the same way. */
export function PlusButton({ open, onPress }: { open: boolean; onPress: () => void }) {
  const opened = useSharedValue(open ? 1 : 0);
  const pressed = useSharedValue(1);
  const pop = useSharedValue(1);
  useEffect(() => {
    opened.set(withTiming(open ? 1 : 0, timing(220)));
    if (open) pop.set(withSequence(withTiming(1.12, timing(120, OVER)), withTiming(1, timing(180))));
  }, [open, opened, pop]);
  const shell = useAnimatedStyle(() => ({
    backgroundColor: interpolateColor(opened.get(), [0, 1], [t.sunken, t.selection]),
    transform: [{ scale: pressed.get() * pop.get() }],
  }));
  const turn = useAnimatedStyle(() => ({ transform: [{ rotate: `${45 * opened.get()}deg` }] }));
  return (
    <Animated.View style={[styles.round, shell]}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={open ? 'Close the attach menu' : 'Attach a photo or file'}
        accessibilityState={{ expanded: open }}
        onPress={onPress}
        onPressIn={() => pressed.set(withSpring(0.9, PRESS))}
        onPressOut={() => pressed.set(withSpring(1, PRESS))}
        style={styles.fill}
      >
        <Animated.View style={turn}>
          <Plus size={20} color={t.text} weight={2.2} />
        </Animated.View>
      </Pressable>
    </Animated.View>
  );
}

/** How long a float takes to go, after which it is gone from the tree. */
const CLOSE_MS = 140;

/** Whether the float has just opened, for its rows to come in after it. */
const Opened = createContext(false);
/** How long after opening a row that mounts still comes in. */
const FRESH_MS = 400;

/** A float over the chat, hung from the composer's top edge: the + menu or
 * the / menu. It grows from its bottom corner, past the mark and back, and
 * drops away faster. Closed, it is out of the tree, so nothing under it
 * takes a tap meant for it. Everything else stays put: the chat, the chips,
 * the field and the keyboard. */
export function ComposerMenu({ open, style, children }: { open: boolean; style?: StyleProp<ViewStyle>; children: ReactNode }) {
  const [shown, setShown] = useState(open);
  const [fresh, setFresh] = useState(open);
  // Opening: in the tree, and fresh, before this render draws it.
  const [was, setWas] = useState(open);
  if (open !== was) {
    setWas(open);
    if (open) {
      setShown(true);
      setFresh(true);
    }
  }
  const strength = useSharedValue(open ? 1 : 0);
  const rise = useSharedValue(open ? 1 : 0);
  useEffect(() => {
    if (open) {
      strength.set(withTiming(1, timing(140)));
      rise.set(withTiming(1, timing(280, OVER)));
      const timer = setTimeout(() => setFresh(false), FRESH_MS);
      return () => clearTimeout(timer);
    }
    strength.set(withTiming(0, timing(110)));
    rise.set(withTiming(0, timing(CLOSE_MS)));
    const timer = setTimeout(() => setShown(false), CLOSE_MS);
    return () => clearTimeout(timer);
  }, [open, strength, rise]);
  const floating = useAnimatedStyle(() => {
    const r = rise.get();
    return { opacity: strength.get(), transform: [{ translateY: 16 * (1 - r) }, { scale: 0.7 + 0.3 * r }] };
  });
  if (!shown) return null;
  return (
    <Opened.Provider value={fresh}>
      <Animated.View pointerEvents={open ? 'auto' : 'none'} style={[styles.float, style, floating]}>
        {children}
      </Animated.View>
    </Opened.Provider>
  );
}

/** A row of a float's, coming up from under the card a beat after the one
 * before it, when the float has just opened; a row that comes later — the
 * / menu narrowing as you type — is simply there. */
export function Stagger({ index, children }: { index: number; children: ReactNode }) {
  const fresh = useContext(Opened);
  const up = useSharedValue(fresh ? 0 : 1);
  useEffect(() => {
    if (!fresh) return;
    up.set(withDelay(index * 40, withTiming(1, timing(300, OVER))));
    // Only on the way in: `fresh` is settled on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const coming = useAnimatedStyle(() => ({ opacity: up.get(), transform: [{ translateY: 14 * (1 - up.get()) }] }));
  return <Animated.View style={coming}>{children}</Animated.View>;
}

/** Where the + menu hangs: off the + button, up to 296 wide. */
export const plusMenu: ViewStyle = { left: 0, right: 0, maxWidth: 296 };
/** Where the / menu hangs: over the field, between the buttons. */
export const slashMenu = (plus: boolean): ViewStyle => ({ left: plus ? size.control + 8 : 0, right: size.control + 8 });

const styles = StyleSheet.create({
  round: { width: size.control, height: size.control, borderRadius: size.control / 2, overflow: 'hidden' },
  send: { backgroundColor: t.selection },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, alignItems: 'center', justifyContent: 'center' },
  glyph: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, alignItems: 'center', justifyContent: 'center' },
  center: { alignItems: 'center', justifyContent: 'center' },
  float: {
    position: 'absolute',
    bottom: '100%',
    marginBottom: 8,
    borderRadius: size.radiusCard,
    borderWidth: 1,
    borderColor: t.border,
    backgroundColor: t.elevated,
    shadowColor: '#000',
    shadowOpacity: 0.55,
    shadowRadius: 20,
    shadowOffset: { width: 0, height: 12 },
    transformOrigin: 'bottom left',
    zIndex: 2,
  },
});
