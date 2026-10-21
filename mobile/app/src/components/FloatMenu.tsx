// A menu floated over the screen: the desktop's `ui::menu`, sized for a
// thumb. The rest of the app dims behind it; a tap there shuts it. It hangs
// under what opened it, or over it when there is no room below, and never
// past the screen's edges.
//
// `lifted` is drawn where the thing held was, above the dimming — a held row
// stays in sight, lifted out of the list, while its menu is open.

import { type ReactNode, useState } from 'react';
import { Modal, Pressable, StyleSheet, Text, View, useWindowDimensions } from 'react-native';
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
export function placeOf(view: View | null | undefined, then: (place: Place) => void): void {
  view?.measureInWindow((x, y, width, height) => then({ x, y, width, height }));
}

export interface MenuItem {
  key: string;
  label: string;
  /** At the row's end, in dim ink — or the danger colour on a danger row. */
  icon?: ReactNode;
  danger?: boolean;
  onPress: () => void;
}

export interface MenuSection {
  /** A dim heading over the section, when it needs a name. */
  title?: string;
  items: MenuItem[];
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
          <View key={section.title ?? at} style={at > 0 && styles.section}>
            {section.title ? <Text style={styles.title}>{section.title}</Text> : null}
            {section.items.map((item) => (
              <Pressable
                key={item.key}
                accessibilityRole="menuitem"
                accessibilityLabel={item.label}
                onPress={() => choose(item)}
                style={({ pressed }) => [styles.item, pressed && { backgroundColor: t.selection }]}
              >
                <Text style={[styles.label, item.danger && { color: t.failed }]} numberOfLines={1}>
                  {item.label}
                </Text>
                {item.icon}
              </Pressable>
            ))}
          </View>
        ))}
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  scrim: { ...StyleSheet.absoluteFill, backgroundColor: tint(t.backdrop, 0.72) },
  lifted: {
    position: 'absolute',
    borderRadius: size.radiusLg,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
    overflow: 'hidden',
    shadowColor: '#000',
    shadowOpacity: 0.6,
    shadowRadius: 20,
    shadowOffset: { width: 0, height: 14 },
  },
  menu: {
    position: 'absolute',
    paddingVertical: 4,
    borderRadius: 14,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
    shadowColor: '#000',
    shadowOpacity: 0.6,
    shadowRadius: 20,
    shadowOffset: { width: 0, height: 14 },
  },
  section: { marginTop: 4, paddingTop: 4, borderTopWidth: 1, borderTopColor: t.border },
  title: { paddingHorizontal: 14, paddingTop: 6, paddingBottom: 2, fontFamily: font.prose, fontSize: 11.5, color: t.dim },
  item: {
    height: size.control,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
    paddingHorizontal: 14,
  },
  label: { flex: 1, fontFamily: font.prose, fontSize: size.body, color: t.text },
});
