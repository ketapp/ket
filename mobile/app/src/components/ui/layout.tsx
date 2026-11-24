// A screen and what it is made of: the bar across its top, cards on the
// desk, ruled cards of rows, headings, figures, prose and banners.

import { router } from 'expo-router';
import { Children, Fragment, isValidElement, type ReactNode } from 'react';
import { StyleSheet, Text, View, type StyleProp, type TextStyle, type ViewStyle } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { READABLE_WIDTH, useSplit } from '../../lib/layout';
import { font, size, t } from '../../lib/theme';
import { surface, tinted } from './surface';
import { Back } from '../icons';
import { IconButton } from './controls';
import { TILE } from './marks';
import { ROW_INSET, TILE_GAP } from './rows';
import { shared } from './shared';

/** A whole screen: the desk, clear of the status bar. On a wide screen its
 * content is a column no wider than reads well, in the middle; `full` gives
 * it the whole width instead, for a terminal. */
export function Screen({ children, style, full }: { children: ReactNode; style?: StyleProp<ViewStyle>; full?: boolean }) {
  const insets = useSafeAreaInsets();
  return (
    <View style={[styles.screen, { paddingTop: insets.top }, style]}>
      {full ? children : <View style={styles.column}>{children}</View>}
    </View>
  );
}

/** The strip along the top, on the desk: back, a title over a detail, actions.
 * `tabs` stands under the detail in the title's place — a {@link TabStrip} —
 * and the bar grows to hold it, Back lined up with the tabs. */
export function Bar({
  back,
  title,
  detail,
  detailColor,
  lead,
  tabs,
  right,
}: {
  back?: boolean;
  title?: ReactNode;
  detail?: ReactNode;
  detailColor?: string;
  lead?: ReactNode;
  tabs?: ReactNode;
  right?: ReactNode;
}) {
  // Beside the sidebar, the first screen of what was picked there has
  // nothing to go back to: the sidebar is where the rest is.
  const split = useSplit();
  const backable = back && !(split && !router.canGoBack());
  return (
    <View style={[styles.bar, tabs ? styles.barTabs : null]}>
      {backable ? (
        <IconButton label="Back" onPress={() => (router.canGoBack() ? router.back() : router.replace('/'))} style={styles.back}>
          <Back size={20} color={t.text} />
        </IconButton>
      ) : null}
      <View style={styles.barText}>
        {detail ? (
          <Text style={[styles.barDetail, detailColor ? { color: detailColor } : null]} numberOfLines={1}>
            {detail}
          </Text>
        ) : null}
        {lead}
        {title ? (
          <Text style={styles.barTitle} numberOfLines={1}>
            {title}
          </Text>
        ) : null}
        {tabs ? <View style={styles.barTabStrip}>{tabs}</View> : null}
      </View>
      {right}
    </View>
  );
}

/** A card: a region of the screen, on the desk. The app's panel: its fill
 * and hairline are the shared stylesheet's. */
export function Card({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[surface.panel, styles.card, style]}>{children}</View>;
}

/** A ruled card's corners: rounder than a card's, as the desk's gutter is
 * wider round it. */
const RULED_RADIUS = 16;

/**
 * A card of rows, each a `ruled` Row, with a rule between each two that
 * starts where the rows' words do, so the tiles stand clear of it.
 */
export function RuledCard({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  const rows = Children.toArray(children);
  return (
    <View style={[surface.panel, styles.ruled, style]}>
      {rows.map((row, at) => (
        // Each row's own key, so a row keeps its state as others come and go.
        <Fragment key={isValidElement(row) && row.key !== null ? row.key : at}>
          {at > 0 ? <View style={styles.rule} /> : null}
          {row}
        </Fragment>
      ))}
    </View>
  );
}

/** A section's name inside a card, with a count at the end, or with
 * `action` there instead: a `Pill` for the one thing the section offers. */
export function Heading({
  children,
  count,
  color,
  action,
}: {
  children: ReactNode;
  count?: ReactNode;
  color?: string;
  action?: ReactNode;
}) {
  return (
    <View style={[styles.heading, action ? styles.headingWithAction : null]}>
      <Text style={[styles.headingText, color ? { color } : null]}>{children}</Text>
      {action ?? (count !== undefined ? <Text style={[shared.count, color ? { color } : null]}>{count}</Text> : null)}
    </View>
  );
}

/** A figure: a dim label over a mono value. */
export function Figure({ label, children, style }: { label: string; children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return (
    <View style={[styles.figure, style]}>
      <Text style={styles.figureLabel} numberOfLines={1}>
        {label}
      </Text>
      <View style={styles.figureValue}>{children}</View>
    </View>
  );
}

/** The text of a figure's value, optionally in a signal's colour. */
export function Value({ children, color }: { children: ReactNode; color?: string }) {
  return <Text style={[styles.value, color ? { color } : null]}>{children}</Text>;
}

/** A read-out: a figure in a small recessed well, usually led by a glyph. */
export function Readout({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[shared.readout, style]}>{children}</View>;
}

/** A message tinted by its signal, as the shared stylesheet tints one. */
export function Banner({ tone, children, style }: { tone: string; children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[styles.banner, tinted(tone), style]}>{children}</View>;
}

/** Prose: a sentence, dim unless told otherwise. */
export function Prose({ children, color, style }: { children: ReactNode; color?: string; style?: StyleProp<TextStyle> }) {
  return <Text style={[styles.prose, color ? { color } : null, style]}>{children}</Text>;
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: t.backdrop },
  column: { flex: 1, width: '100%', maxWidth: READABLE_WIDTH, alignSelf: 'center' },
  bar: {
    minHeight: size.bar,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: size.gutter,
  },
  // Back and the tabs share a bottom edge, both 40 tall, so they line up.
  barTabs: { alignItems: 'flex-end', paddingBottom: 6 },
  barTabStrip: { marginTop: 2 },
  back: { marginLeft: -6 },
  barText: { flex: 1, minWidth: 0, gap: 1 },
  barTitle: { fontFamily: font.proseSemibold, fontSize: size.value, color: t.text },
  barDetail: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  card: { borderRadius: size.radiusCard, overflow: 'hidden' },
  ruled: { borderRadius: RULED_RADIUS, overflow: 'hidden' },
  rule: { height: 1, marginLeft: ROW_INSET + TILE + TILE_GAP, backgroundColor: t.rule },
  heading: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 6,
    paddingTop: 6,
    paddingBottom: 4,
  },
  // Room for a pill, 30 tall, and a finger on it; the pill ends at the
  // card's own padding.
  headingWithAction: { paddingTop: 7, paddingBottom: 7, paddingRight: 0 },
  headingText: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.dim },
  figure: { gap: 3, minWidth: 0 },
  figureLabel: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  figureValue: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  value: { fontFamily: font.chromeMedium, fontSize: size.value, color: t.text },
  prose: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  banner: { borderRadius: size.radiusCard, padding: 12, gap: 10 },
});
