// A sheet: the one way the app puts something over a screen that is not a
// menu hung off what opened it. It comes up from the foot of the screen as
// iOS brings a sheet up — the dimming fades in while the sheet springs up
// from just below the screen — and both go back the way they came. A
// grabber, a title with a line under it and a close button head it; the
// rest scrolls. Every sheet — New task, a session's settings, the Mac
// switcher — is this one, so they all move alike.
//
// `Sheet` is the usual one, over the screen in a modal. `SheetPanel` is the
// same sheet without the modal, for a route that is a sheet itself and
// leaves by navigating once the sheet is down (see app/new/[host].tsx).
//
// The sheet rides the keyboard, frame for frame, for the sheets that hold a
// field, and a tap on the dimming with the keyboard up only puts the
// keyboard away; the next one closes.

import { Children, Fragment, type ReactNode, useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from 'react';
import {
  Dimensions,
  Keyboard,
  Modal,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import Animated, {
  Easing,
  ReduceMotion,
  type SharedValue,
  useAnimatedScrollHandler,
  useAnimatedStyle,
  useDerivedValue,
  useReducedMotion,
  useSharedValue,
  withSpring,
  withTiming,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { READABLE_WIDTH } from '../lib/layout';
import { font, size, t } from '../lib/theme';
import { KeyboardLift, useLift } from './KeyboardLift';
import { Check, Close } from './icons';

/** The dimming's fade in. */
const DIM_MS = 300;
/** The sheet's way in: critically damped, as UIKit springs a sheet up. */
const ARRIVE = { duration: 450, dampingRatio: 1, reduceMotion: ReduceMotion.System } as const;
/** The way out, sheet and dimming together. */
const LEAVE_MS = 260;
const motion = { reduceMotion: ReduceMotion.System };

/** From the sheet's edge to its contents. */
export const SHEET_INSET = 16;
/** The sheet's head — grabber, title and detail — down to where the body
 * starts, for a caller placing something against the body. */
export const SHEET_HEAD = 72;

/** The room a sheet rises in and how tall it came up, for a caller that
 * floats something over the sheet. */
export interface SheetMeasure {
  room: { width: number; height: number };
  height: number;
  /** How far the body's contents are drawn above where they were laid out,
   * frame for frame: scrolled up, and lifted with the sheet on the keyboard.
   * Added to a `bottom` taken from the layout, it holds a thing floated over
   * the sheet against what it came from. */
  rise: SharedValue<number>;
}

export interface SheetProps {
  open: boolean;
  /** Asked to close: the dimming, the close button, the escape gesture. */
  onClose: () => void;
  title: string;
  /** A dim line under the title: whose, or what for. */
  detail?: string;
  /** What the sheet is, for a screen reader; the title when left out. */
  label?: string;
  children: ReactNode;
  /** Drawn in the sheet's room over the sheet — a menu opened from it. */
  overlay?: ReactNode;
  onMeasure?: (measure: SheetMeasure) => void;
}

export function Sheet({ open, onClose, ...rest }: SheetProps) {
  // Up through its way out: the modal hides once the sheet is down.
  const [was, setWas] = useState(open);
  const [down, setDown] = useState(!open);
  if (open !== was) {
    setWas(open);
    if (open) setDown(false);
  }
  return (
    <Modal visible={open || !down} transparent animationType="none" onRequestClose={onClose} statusBarTranslucent>
      <SheetPanel open={open} onClose={onClose} onGone={() => setDown(true)} {...rest} />
    </Modal>
  );
}

export function SheetPanel({
  open,
  onClose,
  onGone,
  title,
  detail,
  label,
  children,
  overlay,
  onMeasure,
}: SheetProps & {
  /** The sheet is down and the dimming gone. */
  onGone?: () => void;
}) {
  const insets = useSafeAreaInsets();
  const still = useReducedMotion();
  const [typing, setTyping] = useState(() => Keyboard.isVisible());
  const [room, setRoom] = useState({ width: 0, height: 0 });
  const [height, setHeight] = useState(0);
  // The same, read by the way out as it starts.
  const tall = useRef(0);
  // The foot keeps the home indicator's room throughout: the keyboard
  // covers that room first, and the sheet only rises for the rest.
  const foot = Math.max(insets.bottom, 12);
  const tucked = foot - 12;

  // Drawn here rather than by a stack or the modal, which only know how to
  // fade or slide the whole thing — dimming and all.
  const dim = useSharedValue(0);
  const drop = useSharedValue(Dimensions.get('window').height);
  const entered = useRef(false);
  const gone = useEffectEvent(() => onGone?.());
  const leaving = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dimming = useAnimatedStyle(() => ({ opacity: dim.get() }));
  const sliding = useAnimatedStyle(() => ({ transform: [{ translateY: drop.get() }] }));
  const scrolled = useSharedValue(0);
  const scrolling = useAnimatedScrollHandler((e) => scrolled.set(e.contentOffset.y));
  const lift = useLift({ opened: tucked });
  const rise = useDerivedValue(() => scrolled.get() - lift.get());

  useEffect(() => {
    if (open) {
      if (leaving.current) clearTimeout(leaving.current);
      leaving.current = null;
      dim.set(withTiming(1, { duration: DIM_MS, ...motion }));
      if (entered.current) drop.set(withSpring(0, ARRIVE));
      return;
    }
    Keyboard.dismiss();
    const leaveMs = still ? 0 : LEAVE_MS;
    dim.set(withTiming(0, { duration: leaveMs, ...motion }));
    drop.set(
      withTiming(tall.current || Dimensions.get('window').height, { duration: leaveMs, easing: Easing.in(Easing.cubic), ...motion }),
    );
    leaving.current = setTimeout(() => {
      leaving.current = null;
      gone();
    }, leaveMs);
  }, [open, dim, drop, still]);

  useEffect(
    () => () => {
      if (leaving.current) clearTimeout(leaving.current);
    },
    [],
  );

  // From the commit, not after paint: a field in the sheet may focus itself
  // as it mounts, and the keyboard's first event must not arrive before
  // anyone listens.
  useLayoutEffect(() => {
    const show = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow', () => setTyping(true));
    const hide = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide', () => setTyping(false));
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  const measured = useEffectEvent((measure: SheetMeasure) => onMeasure?.(measure));
  useEffect(() => {
    measured({ room, height, rise });
  }, [room, height, rise]);

  // Up from just below the screen, once the sheet knows how tall it is.
  const arrive = (measured: number) => {
    tall.current = measured;
    setHeight(measured);
    if (entered.current || !open) return;
    entered.current = true;
    drop.set(measured);
    drop.set(withSpring(0, ARRIVE));
  };

  return (
    <View style={styles.fill}>
      <Animated.View style={[styles.scrim, dimming]}>
        <Pressable
          accessibilityLabel={typing ? 'Hide the keyboard' : 'Dismiss'}
          onPress={() => (typing ? Keyboard.dismiss() : onClose())}
          style={styles.fill}
        />
      </Animated.View>
      {/* Modal to a screen reader, so the dimming — its sibling — is out of
          reach: the escape gesture closes the sheet instead. */}
      <View
        style={styles.fill}
        pointerEvents="box-none"
        accessibilityViewIsModal
        accessibilityLabel={label ?? title}
        onAccessibilityEscape={onClose}
      >
        <View
          style={styles.room}
          pointerEvents="box-none"
          onLayout={(e) => setRoom({ width: e.nativeEvent.layout.width, height: e.nativeEvent.layout.height })}
        >
          <KeyboardLift offset={{ opened: tucked }}>
            <Animated.View
              style={[
                styles.sheet,
                {
                  paddingBottom: foot,
                  maxHeight: room.height ? (typing ? Math.max(300, room.height * 0.58) : room.height - insets.top - 8) : undefined,
                },
                sliding,
              ]}
              onLayout={(e) => arrive(e.nativeEvent.layout.height)}
            >
              <View style={styles.grabber} />
              <View style={styles.head}>
                <View style={styles.headText}>
                  <Text style={styles.title} numberOfLines={1}>
                    {title}
                  </Text>
                  {detail ? (
                    <Text style={styles.detail} numberOfLines={1}>
                      {detail}
                    </Text>
                  ) : null}
                </View>
                <Pressable accessibilityRole="button" accessibilityLabel="Close" onPress={onClose} hitSlop={8} style={styles.close}>
                  <Close size={16} color={t.dim} weight={2} />
                </Pressable>
              </View>
              <Animated.ScrollView
                style={styles.scroll}
                contentContainerStyle={styles.body}
                keyboardShouldPersistTaps="handled"
                bounces={false}
                onScroll={scrolling}
                scrollEventThrottle={16}
              >
                {children}
              </Animated.ScrollView>
            </Animated.View>
          </KeyboardLift>
          {overlay}
        </View>
      </View>
    </View>
  );
}

/** A well of rows in a sheet, a hairline between each. */
export function SheetList({ children }: { children: ReactNode }) {
  return (
    <View style={styles.list}>
      {Children.toArray(children).map((child, at) => (
        <Fragment key={at}>
          {at > 0 ? <View style={styles.rule} /> : null}
          {child}
        </Fragment>
      ))}
    </View>
  );
}

/** A row in a `SheetList`: something to pick, or somewhere to go. The one
 * in force is ticked; `held` dims the rest while a pick goes through. */
export function SheetItem({
  label,
  mono,
  mark,
  detail,
  dot,
  icon,
  on,
  held,
  role = 'button',
  onPress,
}: {
  label: string;
  /** For a label you would quote — a Mac's name — drawn in mono. */
  mono?: boolean;
  /** A glyph before the label. */
  mark?: ReactNode;
  /** A line under the label on what the item is or does. */
  detail?: string;
  /** A dot in this colour before the detail: the item's state. */
  dot?: string;
  /** At the row's end, in place of the tick. */
  icon?: ReactNode;
  /** The row in force now: ticked, and not pressable again. */
  on?: boolean;
  /** Nothing can be picked for now. */
  held?: boolean;
  role?: 'button' | 'radio';
  onPress: () => void;
}) {
  const off = held && !on;
  return (
    <Pressable
      accessibilityRole={role}
      accessibilityLabel={label}
      accessibilityState={role === 'radio' ? { checked: on === true, disabled: held === true } : { selected: on, disabled: held }}
      disabled={held || on}
      onPress={onPress}
      style={({ pressed }) => [styles.item, pressed && { backgroundColor: t.selection }]}
    >
      {mark}
      <View style={styles.words}>
        <Text style={[styles.label, mono && styles.mono, on && styles.labelOn, off && { color: t.faint }]} numberOfLines={1}>
          {label}
        </Text>
        {detail ? (
          <View style={styles.detailRow}>
            {dot ? <View style={[styles.dot, { backgroundColor: dot }]} /> : null}
            <Text style={styles.itemDetail} numberOfLines={2}>
              {detail}
            </Text>
          </View>
        ) : null}
      </View>
      {icon ?? (on ? <Check size={17} color={t.brand} weight={2.6} /> : null)}
    </Pressable>
  );
}

/** A dim line of prose under a sheet's contents: a caveat, where to go. */
export function SheetNote({ children }: { children: ReactNode }) {
  return <Text style={styles.note}>{children}</Text>;
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  scrim: { position: 'absolute', top: 0, right: 0, bottom: 0, left: 0, backgroundColor: 'rgba(0,0,0,0.6)' },
  // On a wide screen the sheet rises from the middle of the foot, no wider
  // than reads well, rather than across the whole screen.
  room: { flex: 1, width: '100%', maxWidth: READABLE_WIDTH, alignSelf: 'center', justifyContent: 'flex-end' },
  sheet: {
    backgroundColor: t.card,
    borderTopLeftRadius: 18,
    borderTopRightRadius: 18,
    borderTopWidth: 1,
    borderColor: t.border,
  },
  grabber: { width: 36, height: 5, borderRadius: 3, backgroundColor: t.border, alignSelf: 'center', marginTop: 8, marginBottom: 6 },
  head: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingTop: 4,
    paddingBottom: 14,
    paddingLeft: SHEET_INSET,
    paddingRight: SHEET_INSET - 4,
  },
  headText: { flex: 1, minWidth: 0, gap: 1 },
  title: { fontFamily: font.proseSemibold, fontSize: size.value, color: t.text },
  detail: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  close: { width: 32, height: 32, borderRadius: 16, backgroundColor: t.sunken, alignItems: 'center', justifyContent: 'center' },
  scroll: { flexShrink: 1 },
  body: { gap: 16, paddingHorizontal: SHEET_INSET, paddingBottom: 4 },
  list: { borderRadius: 12, backgroundColor: t.sunken, paddingVertical: 2 },
  rule: { height: StyleSheet.hairlineWidth, marginLeft: 14, backgroundColor: t.border },
  // A row grows with its detail line; it never clips it.
  item: { minHeight: size.rowSm, paddingVertical: 8, paddingHorizontal: 14, flexDirection: 'row', alignItems: 'center', gap: 10 },
  words: { flex: 1, minWidth: 0, gap: 3 },
  label: { fontFamily: font.prose, fontSize: 15, color: t.text },
  labelOn: { fontFamily: font.proseMedium },
  mono: { fontFamily: font.chrome },
  detailRow: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  itemDetail: { flexShrink: 1, fontFamily: font.prose, fontSize: size.detail, lineHeight: 17, color: t.dim },
  note: { fontFamily: font.prose, fontSize: 12.5, lineHeight: 17, color: t.dim },
});
