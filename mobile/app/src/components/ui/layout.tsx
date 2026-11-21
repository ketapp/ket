// A screen and what it is made of: the bar across its top, cards on the
// desk, headings, figures, prose and banners.

import { router } from 'expo-router';
import { type ReactNode } from 'react';
import { StyleSheet, Text, View, type StyleProp, type TextStyle, type ViewStyle } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { READABLE_WIDTH, useSplit } from '../../lib/layout';
import { font, size, t, tint } from '../../lib/theme';
import { Back } from '../icons';
import { IconButton } from './controls';
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

/** A card: a region of the screen, on the desk. */
export function Card({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[styles.card, style]}>{children}</View>;
}

/** A section's name inside a card, with a count at the end. */
export function Heading({ children, count, color }: { children: ReactNode; count?: ReactNode; color?: string }) {
  return (
    <View style={styles.heading}>
      <Text style={[styles.headingText, color ? { color } : null]}>{children}</Text>
      {count !== undefined ? <Text style={[shared.count, color ? { color } : null]}>{count}</Text> : null}
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

/** A message tinted by its signal: 8% fill, 25% edge. */
export function Banner({ tone, children, style }: { tone: string; children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[styles.banner, { backgroundColor: tint(tone, 0.08), borderColor: tint(tone, 0.25) }, style]}>{children}</View>;
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
  card: {
    backgroundColor: t.card,
    borderWidth: 1,
    borderColor: t.rule,
    borderRadius: size.radiusCard,
    overflow: 'hidden',
  },
  heading: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 6,
    paddingTop: 6,
    paddingBottom: 4,
  },
  headingText: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.dim },
  figure: { gap: 3, minWidth: 0 },
  figureLabel: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  figureValue: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  value: { fontFamily: font.chromeMedium, fontSize: size.value, color: t.text },
  prose: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  banner: { borderWidth: 1, borderRadius: size.radiusCard, padding: 12, gap: 10 },
});
