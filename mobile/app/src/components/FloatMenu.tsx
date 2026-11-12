// The app's one menu: the desktop's `ui::menu`, sized for a thumb. Every
// menu on the phone is drawn here — the same card, the same rows, the same
// way in and out — in one of two places:
//
// - **Over the screen**, for a menu opened from a row or a button: the rest
//   of the app dims behind it and a tap there shuts it. It hangs under what
//   opened it, or over it when there is no room below, and never past the
//   screen's edges. `lifted` is drawn where the thing held was, above the
//   dimming — a held row stays in sight while its menu is open.
// - **In place** (`inline`), for the composer's + and / menus: hung from the
//   top edge of what holds it, over what is above, with nothing dimmed; a
//   tap anywhere else shuts it. A menu over the screen is a modal, and a
//   modal takes the keyboard away; in place, the field keeps it.
//
// Either way it grows from the corner nearest what opened it, past the mark
// and back, its rows coming up a beat apart, and drops away faster. Every
// duration follows Reduce Motion.

import { createContext, type ReactNode, useContext, useEffect, useState } from 'react';
import {
  type LayoutChangeEvent,
  Modal,
  Pressable,
  ScrollView,
  type StyleProp,
  StyleSheet,
  Text,
  View,
  type ViewInstance,
  type ViewStyle,
  useWindowDimensions,
} from 'react-native';
import Animated, {
  Easing,
  ReduceMotion,
  useAnimatedStyle,
  useSharedValue,
  withDelay,
  withTiming,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { font, size, t, tint } from '../lib/theme';

/** Where something is on the screen, as `measureInWindow` reports it. */
export interface Place {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Where `view` is on the screen, handed to `then` once it is known. */
export function placeOf(view: ViewInstance | null | undefined, then: (place: Place) => void): void {
  view?.measureInWindow((x, y, width, height) => then({ x, y, width, height }));
}

export interface MenuItem {
  key: string;
  label: string;
  /** Set for a label you would quote — a Mac's name — drawn in mono. */
  mono?: boolean;
  /** A glyph before the label. */
  mark?: ReactNode;
  /** At the row's end, in dim ink — or the danger colour on a danger row. */
  icon?: ReactNode;
  danger?: boolean;
  /** The row that is in force now: drawn on a raised fill. */
  selected?: boolean;
  /** A line under the label on what the item does. */
  detail?: string;
  /** A dot in this colour before the detail: the item's state. */
  dot?: string;
  onPress: () => void;
}

export interface MenuSection {
  /** A dim heading over the section, when it needs a name. */
  title?: string;
  /** A sentence in place of choices — why there are none — in a row's room. */
  message?: { mark?: ReactNode; text: ReactNode };
  items: MenuItem[];
  /** A sunken line of mono under the items: where to go, what to type. */
  note?: string;
}

/** Room kept between the menu and the screen's edges, and what opened it. */
const EDGE = 12;
const GAP = 8;
/** How wide a menu over the screen is, unless it says otherwise. */
const WIDTH = 280;
/** How tall a menu in place grows before its rows scroll. */
const INLINE_HEIGHT = 248;

/** The one curve: out fast, settling slow. */
const OUT = Easing.bezier(0.2, 0.8, 0.2, 1);
/** Out with a little past the mark, for what opens. */
const OVER = Easing.bezier(0.34, 1.35, 0.64, 1);
const timing = (ms: number, easing = OUT) => ({ duration: ms, easing, reduceMotion: ReduceMotion.System });
const CLOSE_MS = 140;
/** How long after opening a row that mounts still comes in. */
const FRESH_MS = 400;

/** Whether the menu has just opened, for its rows to come in after it. */
const Opened = createContext(false);

/** The card every menu is: it grows from the corner on the side of what
 * opened it — `from` says which — and shrinks away when it closes. `ready`
 * holds the growing back until the card has been placed. */
export function MenuCard({
  open,
  from,
  ready = true,
  label,
  style,
  onLayout,
  children,
}: {
  open: boolean;
  /** Where what opened it is: the card grows away from there. */
  from: 'above' | 'below';
  ready?: boolean;
  label: string;
  style?: StyleProp<ViewStyle>;
  onLayout?: (e: LayoutChangeEvent) => void;
  children: ReactNode;
}) {
  const [fresh, setFresh] = useState(open);
  // Opening: fresh before this render draws it, for its rows to come in.
  const [was, setWas] = useState(open);
  if (open !== was) {
    setWas(open);
    if (open) setFresh(true);
  }
  const strength = useSharedValue(0);
  const rise = useSharedValue(0);
  useEffect(() => {
    if (open && ready) {
      strength.set(withTiming(1, timing(140)));
      rise.set(withTiming(1, timing(280, OVER)));
      const timer = setTimeout(() => setFresh(false), FRESH_MS);
      return () => clearTimeout(timer);
    }
    if (!open) {
      strength.set(withTiming(0, timing(110)));
      rise.set(withTiming(0, timing(CLOSE_MS)));
    }
  }, [open, ready, strength, rise]);
  const away = from === 'above' ? -16 : 16;
  const floating = useAnimatedStyle(() => {
    const r = rise.get();
    return { opacity: strength.get(), transform: [{ translateY: away * (1 - r) }, { scale: 0.7 + 0.3 * r }] };
  });
  return (
    <Opened.Provider value={fresh}>
      <Animated.View
        accessibilityRole="menu"
        accessibilityLabel={label}
        pointerEvents={open ? 'auto' : 'none'}
        onLayout={onLayout}
        style={[styles.menu, { transformOrigin: from === 'above' ? 'top left' : 'bottom left' }, style, floating]}
      >
        {children}
      </Animated.View>
    </Opened.Provider>
  );
}

/** A row of a menu's, coming up from under the card a beat after the one
 * before it when the menu has just opened; a row that comes later — the /
 * menu narrowing as you type — is simply there. */
function Stagger({ index, children }: { index: number; children: ReactNode }) {
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

/** A menu's sections and rows, scrolling past `maxHeight`. `onChoose` is
 * what a row's press does — the caller's, so a menu can shut first. */
export function MenuList({
  sections,
  onChoose,
  maxHeight,
}: {
  sections: MenuSection[];
  onChoose: (item: MenuItem) => void;
  maxHeight?: number;
}) {
  let row = 0;
  const body = sections.map((section, at) => (
    <View key={section.title ?? at}>
      {at > 0 ? <View style={styles.rule} /> : null}
      {section.title ? <Text style={styles.title}>{section.title}</Text> : null}
      {section.message ? (
        <View style={styles.item}>
          {section.message.mark}
          <Text style={[styles.words, styles.message]}>{section.message.text}</Text>
        </View>
      ) : null}
      {section.items.map((item) => (
        <Stagger key={item.key} index={row++}>
          <Pressable
            accessibilityRole="menuitem"
            accessibilityState={item.selected ? { selected: true } : undefined}
            accessibilityLabel={item.label}
            onPress={() => onChoose(item)}
            style={({ pressed }) => [
              styles.item,
              item.selected && { backgroundColor: t.hover },
              pressed && { backgroundColor: t.selection },
            ]}
          >
            {item.mark}
            <View style={styles.words}>
              <Text style={[styles.label, item.mono && styles.mono, item.danger && { color: t.failed }]} numberOfLines={1}>
                {item.label}
              </Text>
              {item.detail ? (
                <View style={styles.detailRow}>
                  {item.dot ? <View style={[styles.dot, { backgroundColor: item.dot }]} /> : null}
                  <Text style={styles.detail} numberOfLines={2}>
                    {item.detail}
                  </Text>
                </View>
              ) : null}
            </View>
            {item.icon}
          </Pressable>
        </Stagger>
      ))}
      {section.note ? <Text style={styles.note}>{section.note}</Text> : null}
    </View>
  ));
  if (maxHeight === undefined) return <View style={styles.list}>{body}</View>;
  return (
    <ScrollView
      style={{ flexGrow: 0, maxHeight }}
      contentContainerStyle={styles.list}
      bounces={false}
      keyboardShouldPersistTaps="handled"
    >
      {body}
    </ScrollView>
  );
}

interface Props {
  open: boolean;
  onClose: () => void;
  sections: MenuSection[];
  /** What the menu is, for a screen reader. */
  label: string;
  /** What opened it, on the screen — for a menu over the screen. */
  anchor?: Place | null;
  /** For a menu over the screen: drawn where the held thing was. */
  lifted?: ReactNode;
  width?: number;
  /** In place rather than over the screen: where along the top edge of what
   * holds it the menu hangs — its `left`, `right` and `maxWidth`. */
  inline?: ViewStyle;
}

/** A menu: over the screen, or in place when `inline` says where. Each row
 * shuts the menu before it does its own thing, so what it opens is not drawn
 * under it. */
export function FloatMenu(props: Props) {
  return props.inline ? <InPlace {...props} hang={props.inline} /> : <OverScreen {...props} />;
}

function InPlace({ open, onClose, sections, label, hang }: Props & { hang: ViewStyle }) {
  const window = useWindowDimensions();
  // Kept in the tree while its close plays, and out of it after, so nothing
  // under a closed menu loses a tap to it.
  const [shown, setShown] = useState(open);
  const [was, setWas] = useState(open);
  if (open !== was) {
    setWas(open);
    if (open) setShown(true);
  }
  useEffect(() => {
    if (open) return;
    const timer = setTimeout(() => setShown(false), CLOSE_MS);
    return () => clearTimeout(timer);
  }, [open]);
  if (!shown) return null;
  const choose = (item: MenuItem) => {
    onClose();
    item.onPress();
  };
  return (
    <>
      {/* Clear, and a screen past what holds the menu every way, so a tap
          anywhere but the menu shuts it. Gone as the close starts, so that
          tap is the only one it takes. */}
      {open ? (
        <Pressable
          accessibilityLabel="Close menu"
          onPress={onClose}
          style={[styles.catcher, { top: -window.height, bottom: -window.height, left: -window.width, right: -window.width }]}
        />
      ) : null}
      <MenuCard open={open} from="below" label={label} style={[styles.inline, hang]}>
        <MenuList sections={sections} onChoose={choose} maxHeight={INLINE_HEIGHT} />
      </MenuCard>
    </>
  );
}

function OverScreen({ open, onClose, anchor, sections, lifted, label, width = WIDTH }: Props) {
  const screen = useWindowDimensions();
  const insets = useSafeAreaInsets();
  // Measured afresh for each opening: the next menu may hold more, or less.
  const [sized, setSized] = useState<{ anchor: Place; height: number } | null>(null);
  if (!anchor) return null;
  const height = sized?.anchor === anchor ? sized.height : 0;

  const top = insets.top + EDGE;
  const bottom = screen.height - insets.bottom - EDGE;
  const below = anchor.y + anchor.height + GAP;
  const above = anchor.y - GAP - height;
  const fitsBelow = below + height <= bottom;
  const y = fitsBelow ? below : above >= top ? above : Math.max(top, bottom - height);
  const left = Math.max(EDGE, Math.min(anchor.x, screen.width - EDGE - width));

  const choose = (item: MenuItem) => {
    onClose();
    item.onPress();
  };

  return (
    <Modal transparent visible={open} animationType="fade" onRequestClose={onClose} statusBarTranslucent>
      <Pressable accessibilityLabel="Close menu" onPress={onClose} style={styles.scrim} />
      {lifted ? (
        <View
          pointerEvents="none"
          style={[styles.lifted, { left: anchor.x, top: anchor.y, width: anchor.width, minHeight: anchor.height }]}
        >
          {lifted}
        </View>
      ) : null}
      <View style={{ position: 'absolute', left, top: y, width, opacity: height > 0 ? 1 : 0 }}>
        <MenuCard
          open={open}
          from={fitsBelow ? 'above' : 'below'}
          ready={height > 0}
          label={label}
          onLayout={(e) => setSized({ anchor, height: e.nativeEvent.layout.height })}
        >
          <MenuList sections={sections} onChoose={choose} maxHeight={bottom - top} />
        </MenuCard>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  scrim: { ...StyleSheet.absoluteFill, backgroundColor: tint(t.backdrop, 0.72) },
  // No edge: it sits exactly on the row's measured frame, and a border
  // inside that frame moved everything in it a pixel as the menu opened.
  lifted: {
    position: 'absolute',
    borderRadius: size.radiusLg,
    backgroundColor: t.elevated,
    overflow: 'hidden',
  },
  // The desktop's menu: a 6px frame, rows with their own corners inside it,
  // a hairline inset from the edges between sections. The shadow lifts it
  // off a chat that is not dimmed under it.
  menu: {
    borderRadius: size.radiusLg,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
    shadowColor: '#000',
    shadowOpacity: 0.55,
    shadowRadius: 20,
    shadowOffset: { width: 0, height: 12 },
  },
  // Hung off the top of what holds it, over everything above.
  inline: { position: 'absolute', bottom: '100%', marginBottom: GAP, zIndex: 2 },
  catcher: { position: 'absolute', zIndex: 2 },
  list: { padding: 6 },
  rule: { height: 1, marginVertical: 6, marginHorizontal: 4, backgroundColor: t.border },
  title: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6, fontFamily: font.chromeMedium, fontSize: 11.5, color: t.dim },
  // A row grows with its detail line; it never clips it.
  item: {
    minHeight: size.control,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 11,
    paddingVertical: 9,
    paddingHorizontal: 10,
    borderRadius: size.radius,
  },
  words: { flex: 1, gap: 3 },
  label: { fontFamily: font.prose, fontSize: 15, color: t.text },
  mono: { fontFamily: font.chrome },
  message: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  detailRow: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  detail: { flexShrink: 1, fontFamily: font.prose, fontSize: size.detail, lineHeight: 17, color: t.dim },
  note: {
    marginTop: 6,
    marginHorizontal: 4,
    marginBottom: 4,
    paddingVertical: 9,
    paddingHorizontal: 10,
    borderRadius: size.radius,
    backgroundColor: t.sunken,
    fontFamily: font.chrome,
    fontSize: size.caption,
    lineHeight: 16.5,
    color: t.dim,
  },
});
