// A menu floated over the screen: the desktop's `ui::menu`, sized for a
// thumb. The rest of the app dims behind it; a tap there shuts it. It hangs
// under what opened it, or over it when there is no room below, and never
// past the screen's edges.
//
// `lifted` is drawn where the thing held was, above the dimming — a held row
// stays in sight, lifted out of the list, while its menu is open.

import { type ReactNode, useState } from 'react';
import { Modal, Pressable, StyleSheet, Text, View, type ViewInstance, useWindowDimensions } from 'react-native';
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
  items: MenuItem[];
  /** A sunken line of mono under the items: where to go, what to type. */
  note?: string;
}

/** Room kept between the menu and the screen's edges, and what opened it. */
const EDGE = 12;
const GAP = 8;

export function FloatMenu({
  open,
  onClose,
  anchor,
  sections,
  lifted,
  label,
  width = 280,
}: {
  open: boolean;
  onClose: () => void;
  anchor: Place | null;
  sections: MenuSection[];
  lifted?: ReactNode;
  /** What the menu is, for a screen reader. */
  label: string;
  width?: number;
}) {
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
  const y = below + height <= bottom ? below : above >= top ? above : Math.max(top, bottom - height);
  const left = Math.max(EDGE, Math.min(anchor.x, screen.width - EDGE - width));

  // Each item shuts the menu first, so what it opens is not drawn under it.
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
      <View
        accessibilityRole="menu"
        accessibilityLabel={label}
        onLayout={(e) => setSized({ anchor, height: e.nativeEvent.layout.height })}
        style={[styles.menu, { left, top: y, width, opacity: height > 0 ? 1 : 0 }]}
      >
        {sections.map((section, at) => (
          <View key={section.title ?? at}>
            {at > 0 ? <View style={styles.rule} /> : null}
            {section.title ? <Text style={styles.title}>{section.title}</Text> : null}
            {section.items.map((item) => (
              <Pressable
                key={item.key}
                accessibilityRole="menuitem"
                accessibilityState={item.selected ? { selected: true } : undefined}
                accessibilityLabel={item.label}
                onPress={() => choose(item)}
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
            ))}
            {section.note ? <Text style={styles.note}>{section.note}</Text> : null}
          </View>
        ))}
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
  // a hairline inset from the edges between sections.
  menu: {
    position: 'absolute',
    padding: 6,
    borderRadius: size.radiusLg,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
  },
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
