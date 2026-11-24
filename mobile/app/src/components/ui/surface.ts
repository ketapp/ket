// The app's one stylesheet for what a surface is filled with and edged in.
// Every panel, float, pill, chip and outlined control takes its fill and its
// hairline from here, so they change together and none picks a grey of its
// own. The rule: an edge is one step up the ramp from the fill it outlines
// (see `ramp` in lib/theme.ts), which keeps it quiet on every theme.
//
// A screen sizes, rounds and pads its own surfaces; it spreads one of these
// in for the look: `card: { ...surface.panel, borderRadius: 12, padding: 4 }`.

import { StyleSheet, type ViewStyle } from 'react-native';

import { blend, t, tint } from '../../lib/theme';

export const surface = StyleSheet.create({
  /** A panel on the ground: a card, a sheet, a prompt. */
  panel: { backgroundColor: t.card, borderWidth: 1, borderColor: t.rule },
  /** What floats over the screen or stands on it with an edge of its own:
   * the tab bar, a menu, a capsule, a pill, a chip, a toggle's track. */
  float: { backgroundColor: t.elevated, borderWidth: 1, borderColor: t.selection },
  /** The same fill and hairline for a surface sized without an edge — a
   * well, a field, a track, a key: the hairline is an outline drawn just
   * inside it, which takes no room, so the surface stays the size it is. */
  well: { backgroundColor: t.sunken, outlineWidth: 1, outlineOffset: -1, outlineColor: t.selection },
  /** What is chosen, or under a finger. */
  chosen: { backgroundColor: t.selection },
  /** An edge and no fill: a secondary button, an option, a key. */
  outline: { borderWidth: 1, borderColor: t.border },
});

/** A surface tinted by a signal — waiting, failed: an 8% fill and a 14% edge
 * of its colour. */
export function tinted(tone: string): ViewStyle {
  return { backgroundColor: tint(tone, 0.08), borderWidth: 1, borderColor: tint(tone, 0.14) };
}

/** The same tint as one solid colour, for a surface that rides over other
 * things, which must not show through it. */
export function tintedSolid(tone: string): ViewStyle {
  return { backgroundColor: blend(t.backdrop, tone, 0.1), borderWidth: 1, borderColor: blend(t.backdrop, tone, 0.2) };
}
