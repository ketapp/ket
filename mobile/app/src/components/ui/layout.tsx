// A screen and what it is made of: the bar across its top, cards on the
// desk, ruled cards of rows, headings, figures, prose and banners.

import { router } from 'expo-router';
import { Children, Fragment, isValidElement, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View, type StyleProp, type TextStyle, type ViewStyle } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { READABLE_WIDTH, useSplit } from '../../lib/layout';
import { font, size, t } from '../../lib/theme';
import { PRESSED, surface, tinted } from './surface';
import { Back, ChevronDown } from '../icons';
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

/**
 * The head of a tab, or of a screen a tab opens: a large title, a quiet line
 * under it, and the one action across from them. `onTitle` makes the title a
 * choice — a chevron after it, a tap opening the choice; `titleMark` stands
 * before it. A screen opened from a tab has `onBack`: a back arrow over the
 * title. A string `line` is drawn in the line's own type; anything else is
 * drawn as given, in `shared.headLine` where it has words.
 */
export function Masthead({
  title,
  titleMark,
  titleLabel,
  onTitle,
  line,
  action,
  onBack,
}: {
  title: string;
  titleMark?: ReactNode;
  /** What a screen reader calls the title when it is a choice. */
  titleLabel?: string;
  onTitle?: () => void;
  line?: ReactNode;
  action?: ReactNode;
  onBack?: () => void;
}) {
  const heading = (
    <>
      {titleMark ? <View style={styles.mastMark}>{titleMark}</View> : null}
      <Text style={styles.mastTitle} numberOfLines={1}>
        {title}
      </Text>
      {onTitle ? <ChevronDown size={14} color={t.faint} weight={2.4} /> : null}
    </>
  );
  return (
    <View>
      {onBack ? (
        <View style={styles.mastBack}>
          <IconButton label="Back" onPress={onBack} style={styles.back}>
            <Back size={22} color={t.text} />
          </IconButton>
        </View>
      ) : null}
      <View style={[styles.mast, onBack ? styles.mastUnderBack : null]}>
        <View style={styles.mastText}>
          {onTitle ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={titleLabel}
              onPress={onTitle}
              hitSlop={6}
              style={({ pressed }) => [styles.mastTitleRow, pressed && { opacity: PRESSED }]}
            >
              {heading}
            </Pressable>
          ) : (
            <View accessibilityRole="header" style={styles.mastTitleRow}>
              {heading}
            </View>
          )}
          {typeof line === 'string' ? (
            <Text style={shared.headLine} numberOfLines={1}>
              {line}
            </Text>
          ) : (
            line
          )}
        </View>
        {action}
      </View>
    </View>
  );
}

/** What the rows of the card above mean, quiet, under it; led by `icon`
 * where one says it at a glance — the lock on "end-to-end encrypted". */
export function CardNote({ children, icon }: { children: ReactNode; icon?: ReactNode }) {
  if (!icon) return <Text style={styles.cardNote}>{children}</Text>;
  return (
    <View style={styles.cardNoteRow}>
      <View style={styles.cardNoteIcon}>{icon}</View>
      <Text style={[styles.cardNote, styles.cardNoteText]}>{children}</Text>
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

/** A screen with nothing on it yet: a quiet mark in a well, centred in the
 * room it is given, its title and what would fill it. */
export function Empty({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <View style={styles.empty} accessibilityLiveRegion="polite">
      <View style={styles.emptyMark}>{icon}</View>
      <Text style={styles.emptyTitle}>{title}</Text>
      <Prose style={styles.emptyNote}>{children}</Prose>
    </View>
  );
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
  // The title and the line under it, and the action across from them, at the
  // desk's edge.
  mast: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 12, paddingTop: 10, paddingHorizontal: 16 },
  mastUnderBack: { paddingTop: 0 },
  mastBack: { height: 44, flexDirection: 'row', alignItems: 'center', paddingHorizontal: 10 },
  mastText: { flexShrink: 1, minWidth: 0, gap: 3 },
  mastTitleRow: { alignSelf: 'flex-start', maxWidth: '100%', flexDirection: 'row', alignItems: 'center', gap: 7 },
  mastMark: { marginRight: 2 },
  mastTitle: { flexShrink: 1, fontFamily: font.proseSemibold, fontSize: 22, lineHeight: 28, letterSpacing: -0.3, color: t.text },
  cardNote: { marginTop: 8, paddingHorizontal: 4, fontFamily: font.prose, fontSize: size.detail, lineHeight: 18, color: t.faint },
  cardNoteRow: { flexDirection: 'row', alignItems: 'flex-start', gap: 8, marginTop: 8, paddingHorizontal: 4 },
  cardNoteIcon: { marginTop: 3 },
  cardNoteText: { flex: 1, marginTop: 0, paddingHorizontal: 0 },
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
  empty: { flexGrow: 1, alignItems: 'center', justifyContent: 'center', gap: 6, paddingHorizontal: 28, paddingBottom: 40 },
  emptyMark: {
    width: 44,
    height: 44,
    marginBottom: 8,
    borderRadius: size.radiusLg,
    alignItems: 'center',
    justifyContent: 'center',
    ...surface.well,
  },
  emptyTitle: { fontFamily: font.proseSemibold, fontSize: size.title, color: t.text, textAlign: 'center' },
  emptyNote: { textAlign: 'center' },
});
