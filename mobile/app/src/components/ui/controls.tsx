// What a person presses or sets: buttons, a segmented track, a row of tabs,
// a stat filter, toggles, choices, a meter, and a text field's well.

import { Fragment, type ReactNode } from 'react';
import {
  Platform,
  Pressable,
  StyleSheet,
  Text,
  View,
  type StyleProp,
  TouchableOpacity,
  type TextStyle,
  type ViewStyle,
} from 'react-native';
import Animated, { Easing, Keyframe, ReduceMotion } from 'react-native-reanimated';

import { font, size, t, tint } from '../../lib/theme';
import { PRESSED, surface } from './surface';
import { Check } from '../icons';
import { Dot } from './marks';
import { shared } from './shared';

/** One answer in a segmented track. */
export interface Segment<K extends string> {
  key: K;
  label: string;
  count?: number;
  /** A mark before the label. */
  icon?: ReactNode;
  /** What a screen reader calls it, for a segment with no label of its own. */
  name?: string;
}

/**
 * A segmented track: a sunken well, the chosen segment raised. A filter
 * shares the width; a count rides in the label a step under it.
 */
export function Segmented<K extends string>({
  segments,
  value,
  onChange,
  disabled,
  tall,
  height,
  round,
  style,
}: {
  segments: Segment<K>[];
  value: K;
  onChange: (key: K) => void;
  /** Kept in place at 55%, as a disabled button is. */
  disabled?: boolean;
  /** As tall as a field, for a track set among fields. */
  tall?: boolean;
  /** The segments' own height, to stand as tall as a button beside the
   * track: the track is 6 more. */
  height?: number;
  /** A capsule, track and segments, for a track beside a capsule button. */
  round?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  const segmentHeight = height ?? (tall ? size.control - 6 : 30);
  return (
    <View style={[styles.track, round && { borderRadius: (segmentHeight + 6) / 2 }, disabled && { opacity: 0.55 }, style]}>
      {segments.map((segment) => {
        const on = segment.key === value;
        return (
          <Pressable
            key={segment.key}
            accessibilityRole="button"
            accessibilityLabel={segment.name}
            accessibilityState={{ selected: on, disabled }}
            disabled={disabled}
            onPress={() => onChange(segment.key)}
            style={[
              styles.segment,
              tall && styles.segmentTall,
              height !== undefined && { height },
              round && { borderRadius: segmentHeight / 2 },
              on && shared.segmentOn,
            ]}
          >
            {segment.icon}
            {segment.label ? (
              <Text style={[styles.segmentText, on && { color: t.text }]} numberOfLines={1}>
                {segment.label}
              </Text>
            ) : null}
            {segment.count !== undefined ? (
              <Text style={[styles.segmentCount, on && { color: t.dim }]}>{segment.count}</Text>
            ) : null}
          </Pressable>
        );
      })}
    </View>
  );
}

/** One tab in a {@link Tabs} row. */
export interface Tab<K extends string> {
  key: K;
  label: string;
  count?: number;
  /** A state's own colour, which the tab wears chosen: its underline and its
   * count. A tab with no state of its own — "All" — wears the text's. */
  tone?: string;
  /** Its icon, drawn chosen or not: an icon fills while its tab is chosen. */
  icon?: (on: boolean) => ReactNode;
  /** What a screen reader calls it, count and all. */
  name?: string;
}

/**
 * A row of tabs over a hairline: each an icon, a word and a count, the chosen
 * one in the text's colour over a 2 pt underline in its tone. For a filter at
 * the top of a list, where a track's well would be one more box.
 */
export function Tabs<K extends string>({
  tabs,
  value,
  onChange,
  style,
}: {
  tabs: Tab<K>[];
  value: K;
  onChange: (key: K) => void;
  style?: StyleProp<ViewStyle>;
}) {
  return (
    <View accessibilityRole="tablist" style={[styles.tabs, style]}>
      <View style={styles.tabRow}>
        {tabs.map((tab) => {
          const on = tab.key === value;
          return (
            <Pressable
              key={tab.key}
              accessibilityRole="tab"
              accessibilityLabel={tab.name}
              accessibilityState={{ selected: on }}
              onPress={() => onChange(tab.key)}
              hitSlop={{ left: 9, right: 9 }}
              style={({ pressed }) => [
                styles.tab,
                { borderBottomColor: on ? (tab.tone ?? t.text) : 'transparent' },
                pressed && !on && { opacity: 0.6 },
              ]}
            >
              {tab.icon?.(on)}
              <Text style={[styles.tabText, on && styles.tabTextOn]} numberOfLines={1}>
                {tab.label}
              </Text>
              {tab.count !== undefined ? (
                <Text style={[styles.tabCount, on && { fontFamily: font.chromeSemibold, color: tab.tone ?? t.dim }]}>
                  {tab.count}
                </Text>
              ) : null}
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}

/** One tile in a {@link StatFilter}: a count that is also a filter. */
export interface Stat<K extends string> {
  key: K;
  label: string;
  count: number;
  /** The dot beside the count, in the state's own colour. Left out for a
   * tile with no single state of its own, such as "All". */
  color?: string;
}

/**
 * A row of figures that doubles as a filter: the resting state already is
 * the summary, so a screen that would otherwise show the same count twice
 * — once as a figure, once as a filter's label — shows it once.
 */
export function StatFilter<K extends string>({
  stats,
  value,
  onChange,
  style,
}: {
  stats: Stat<K>[];
  value: K;
  onChange: (key: K) => void;
  style?: StyleProp<ViewStyle>;
}) {
  return (
    <View style={[styles.statsRow, style]}>
      {stats.map((stat) => {
        const on = stat.key === value;
        const zero = stat.count === 0;
        return (
          <TouchableOpacity
            key={stat.key}
            accessibilityRole="button"
            accessibilityState={{ selected: on }}
            accessibilityLabel={`${stat.label}, ${stat.count}`}
            onPress={() => onChange(stat.key)}
            activeOpacity={on ? 1 : PRESSED}
            style={[styles.stat, on && styles.statOn]}
          >
            <Text style={styles.statLabel} numberOfLines={1}>
              {stat.label}
            </Text>
            <View style={styles.statValue}>
              {stat.color ? <Dot color={zero ? t.border : stat.color} size={7} /> : null}
              <Text style={[styles.statNum, zero && { color: t.dim }]}>{stat.count}</Text>
            </View>
          </TouchableOpacity>
        );
      })}
    </View>
  );
}

/**
 * A small action beside a heading or in a bar: a capsule on the float's
 * fill, its glyph before its word. For what a surface offers but is not
 * there for — pairing another desktop from the list of them. Smaller than a
 * finger, so it takes touches from the room around it. `label` is what a
 * screen reader calls it when its word alone would not say.
 */
export function Pill({
  title,
  icon,
  label,
  tone,
  onPress,
}: {
  title: string;
  icon?: ReactNode;
  label?: string;
  /** A signal's own action, in a row that carries it — Answer, on what
   * waits on you: 28 tall, on its colour at 18%, its word in it. */
  tone?: string;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label ?? title}
      onPress={onPress}
      hitSlop={tone ? 8 : 7}
      style={({ pressed }) =>
        tone
          ? [styles.pillToned, { backgroundColor: tint(tone, pressed ? 0.28 : 0.18) }]
          : [styles.pill, pressed && { opacity: PRESSED }]
      }
    >
      {icon}
      <Text style={[styles.pillText, tone ? [styles.pillTextToned, { color: tone }] : null]}>{title}</Text>
    </Pressable>
  );
}

/** Ink on a `warn` button. */
export const ON_WARN = '#1b0f03';

/** A button's mark landing over its words: a little pop. */
const MARK_IN = new Keyframe({
  0: { opacity: 0, transform: [{ scale: 0.4 }] },
  100: { opacity: 1, transform: [{ scale: 1 }], easing: Easing.bezier(0.34, 1.56, 0.64, 1) },
})
  .duration(240)
  .reduceMotion(ReduceMotion.System);

export function Button({
  title,
  onPress,
  kind = 'secondary',
  icon,
  mark,
  disabled,
  capsule,
  style,
}: {
  title: string;
  onPress: () => void;
  /** `warn`: the call to action on a warm sheet — what to do now an agent
   * has stopped — in the quota's colour. */
  kind?: 'primary' | 'brand' | 'secondary' | 'ghost' | 'danger' | 'warn';
  icon?: ReactNode;
  /** Pressed, and what it did is done: its mark stands over its words, at
   * full strength, and it takes no more presses. */
  mark?: ReactNode;
  disabled?: boolean;
  /** The system's capsule, 40 tall, for a row of answers in a card: a
   * secondary one on the raised fill with its hairline, a danger one on its
   * colour at 14%. */
  capsule?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  const marked = mark != null;
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled || marked}
      style={({ pressed }) => [
        styles.button,
        kind === 'primary' && styles.primary,
        kind === 'brand' && styles.brand,
        kind === 'secondary' && styles.secondary,
        kind === 'danger' && styles.danger,
        kind === 'warn' && styles.warn,
        capsule && styles.capsule,
        capsule && kind === 'secondary' && surface.float,
        capsule && kind === 'danger' && styles.capsuleDanger,
        pressed &&
          (kind === 'primary'
            ? { backgroundColor: t.accentPressed }
            : kind === 'brand'
              ? { backgroundColor: '#9aeaa4' }
              : { opacity: PRESSED }),
        disabled && !marked && { opacity: 0.55 },
        style,
      ]}
    >
      {marked && icon ? <View style={styles.unseen}>{icon}</View> : icon}
      <Text
        style={[
          styles.buttonText,
          kind === 'primary' && { color: t.onAccent, fontFamily: font.proseSemibold },
          kind === 'brand' && styles.brandText,
          kind === 'ghost' && { color: t.dim },
          kind === 'danger' && { color: t.failed },
          capsule && kind === 'danger' && { fontFamily: font.proseSemibold },
          kind === 'warn' && styles.warnText,
          marked && styles.unseen,
        ]}
      >
        {title}
      </Text>
      {marked ? (
        <Animated.View entering={MARK_IN} pointerEvents="none" style={styles.buttonMark}>
          {mark}
        </Animated.View>
      ) : null}
    </Pressable>
  );
}

export function IconButton({
  label,
  onPress,
  children,
  well,
  style,
}: {
  label: string;
  onPress: () => void;
  children: ReactNode;
  /** Set in a sunken well, for an icon button standing alone on a card. */
  well?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  return (
    <TouchableOpacity
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      hitSlop={4}
      activeOpacity={PRESSED}
      style={[styles.iconButton, well && { backgroundColor: t.sunken }, style]}
    >
      {children}
    </TouchableOpacity>
  );
}

/**
 * Related glyph buttons in one frame — the desktop's `icon_cluster`: the
 * mode pill's edge, fill and height, touching cells with a hairline between.
 * The cells are 34 tall; hit-slop makes up the rest of the touch target.
 * A cell with a `badge` wears a dot at its glyph's corner; the badge is what
 * the dot means, said after the label.
 */
export function IconCluster({
  actions,
}: {
  actions: { label: string; onPress: () => void; icon: ReactNode; badge?: string }[];
}) {
  return (
    <View style={styles.cluster}>
      {actions.map((action, index) => (
        <Fragment key={action.label}>
          {index > 0 ? <View style={styles.clusterRule} /> : null}
          <TouchableOpacity
            accessibilityRole="button"
            accessibilityLabel={action.badge ? `${action.label}, ${action.badge}` : action.label}
            onPress={action.onPress}
            hitSlop={{ top: 5, bottom: 5 }}
            activeOpacity={PRESSED}
            style={styles.clusterCell}
          >
            {action.icon}
            {action.badge ? <View style={styles.clusterBadge} /> : null}
          </TouchableOpacity>
        </Fragment>
      ))}
    </View>
  );
}

/**
 * A meter: how full something is, as a length on a rounded track — the
 * desktop's `ui::progress`. `fraction` is 0 to 1.
 */
export function Meter({
  fraction,
  color = t.accent,
  height = 6,
  style,
}: {
  fraction: number;
  color?: string;
  height?: number;
  style?: StyleProp<ViewStyle>;
}) {
  const share = Number.isFinite(fraction) ? Math.min(1, Math.max(0, fraction)) : 0;
  return (
    <View style={[styles.meter, { height, borderRadius: height / 2 }, style]}>
      {share > 0 ? (
        <View style={{ width: `${share * 100}%`, minWidth: height, height, borderRadius: height / 2, backgroundColor: color }} />
      ) : null}
    </View>
  );
}

/** A switch: on is the accent track, off a well with an edge. */
export function Toggle({ on, onChange, label }: { on: boolean; onChange: (on: boolean) => void; label: string }) {
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityLabel={label}
      accessibilityState={{ checked: on }}
      onPress={() => onChange(!on)}
      hitSlop={6}
      style={[styles.toggle, on ? styles.toggleOn : null]}
    >
      <View style={[styles.knob, on ? styles.knobOn : null]} />
    </Pressable>
  );
}

/**
 * One option to pick: a round mark when only one can be, a square one when
 * any number can, then its label over what it means. Picked, the mark fills
 * with the accent and the row takes the selection fill.
 */
export function Choice({
  many,
  on,
  label,
  description,
  onPress,
  disabled,
}: {
  many?: boolean;
  on: boolean;
  label: string;
  description?: string;
  onPress: () => void;
  disabled?: boolean;
}) {
  return (
    <TouchableOpacity
      accessibilityRole={many ? 'checkbox' : 'radio'}
      accessibilityLabel={label}
      accessibilityState={{ checked: on, disabled }}
      onPress={onPress}
      disabled={disabled}
      activeOpacity={PRESSED}
      style={[styles.choice, on && { backgroundColor: t.selection }, disabled && { opacity: 0.55 }]}
    >
      <View
        style={[
          styles.choiceMark,
          many ? { borderRadius: size.radiusSm } : { borderRadius: 9 },
          on && { borderColor: t.accent, backgroundColor: many ? t.accent : 'transparent' },
        ]}
      >
        {on ? many ? <Check size={13} color={t.onAccent} weight={2.6} /> : <View style={styles.choiceDot} /> : null}
      </View>
      <View style={shared.rowText}>
        <Text style={styles.choiceLabel}>{label}</Text>
        {description ? <Text style={shared.detail}>{description}</Text> : null}
      </View>
    </TouchableOpacity>
  );
}

/**
 * For a one-line `TextInput` with a placeholder. React Native 0.88 hands a
 * field the native view of one that has gone, and one taken over from a
 * multiline field with a `lineHeight` (the chat's composer) keeps that line
 * height on its placeholder, which then sits low. A line-break mode gives
 * the placeholder a paragraph style of its own (react-native#59132, fixed
 * after 0.88.0).
 */
export const oneLine = { lineBreakModeIOS: 'tail' } as const;

/** A text field's well, for a `TextInput`. */
export const field: TextStyle = {
  height: size.control,
  borderRadius: size.radius,
  ...surface.well,
  paddingHorizontal: 12,
  color: t.text,
  fontFamily: font.prose,
  fontSize: size.body,
};

const styles = StyleSheet.create({
  meter: { backgroundColor: t.border, overflow: 'hidden' },
  track: { flexDirection: 'row', gap: 2, padding: 3, ...surface.well, borderRadius: size.radius },
  segment: {
    flex: 1,
    height: 30,
    borderRadius: size.radiusSm,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    paddingHorizontal: 4,
  },
  segmentTall: { height: size.control - 6 },
  segmentText: { fontFamily: font.proseMedium, fontSize: size.label, color: t.dim },
  segmentCount: { fontFamily: font.chrome, fontSize: 11, color: t.faint },
  statsRow: { flexDirection: 'row', gap: 4 },
  stat: { flex: 1, minWidth: 0, gap: 3, paddingVertical: 7, paddingHorizontal: 9, borderRadius: size.radiusRow },
  statOn: { backgroundColor: t.selection },
  statLabel: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  statValue: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  statNum: { fontFamily: font.chromeMedium, fontSize: size.value, color: t.text },
  button: {
    height: size.control,
    borderRadius: size.radius,
    paddingHorizontal: 16,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
  },
  pill: {
    height: 30,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    paddingLeft: 9,
    paddingRight: 12,
    borderRadius: 15,
    ...surface.float,
  },
  pillText: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.text },
  pillToned: { height: 28, flexDirection: 'row', alignItems: 'center', gap: 5, paddingHorizontal: 12, borderRadius: 14 },
  pillTextToned: { fontFamily: font.proseSemibold },
  // The tabs' hairline runs the screen's width; the tabs sit on it, in from
  // its edges, their underlines over it.
  tabs: { borderBottomWidth: 1, borderBottomColor: t.rule },
  tabRow: { flexDirection: 'row', gap: 18, paddingHorizontal: 16 },
  tab: {
    height: 40,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    borderBottomWidth: 2,
    marginBottom: -1,
  },
  tabText: { fontFamily: font.chromeMedium, fontSize: size.body, color: t.faint },
  tabTextOn: { fontFamily: font.chromeSemibold, color: t.text },
  tabCount: { fontFamily: font.chromeMedium, fontSize: 12, color: t.faint, fontVariant: ['tabular-nums'] },
  primary: { backgroundColor: t.accent },
  /** The one call to action on a screen that has nothing else yet: green, tall. */
  brand: {
    height: 54,
    gap: 10,
    backgroundColor: t.brand,
  },
  // The system's own sans, not a brand face: the one call to action reads
  // as the phone's.
  brandText: {
    color: t.onBrand,
    fontFamily: Platform.select({ ios: 'System', default: 'sans-serif' }),
    fontWeight: '600',
    fontSize: 16,
  },
  secondary: { ...surface.outline },
  danger: { borderWidth: 1, borderColor: tint(t.failed, 0.45) },
  warn: { backgroundColor: t.quotaHot },
  capsule: { height: 40, borderRadius: 20 },
  capsuleDanger: { borderWidth: 0, backgroundColor: tint(t.failed, 0.14) },
  // Ink on the quota's colour, in either theme: it is orange in both.
  warnText: { color: ON_WARN, fontFamily: font.proseSemibold },
  buttonText: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  // Over the words it stands for, which keep the button its width.
  buttonMark: { ...StyleSheet.absoluteFill, alignItems: 'center', justifyContent: 'center' },
  unseen: { opacity: 0 },
  iconButton: { width: 40, height: 40, borderRadius: size.radius, alignItems: 'center', justifyContent: 'center' },
  cluster: {
    height: 34,
    flexDirection: 'row',
    alignItems: 'stretch',
    borderRadius: 17,
    ...surface.float,
    overflow: 'hidden',
  },
  clusterRule: { width: 1, backgroundColor: t.border },
  clusterCell: { width: 40, alignItems: 'center', justifyContent: 'center' },
  // Over the 18 pt glyph's top-right corner, ringed in the cluster's fill so
  // it stands off the strokes under it.
  clusterBadge: {
    position: 'absolute',
    top: 5,
    right: 7,
    width: 9,
    height: 9,
    borderRadius: 4.5,
    borderWidth: 1.5,
    borderColor: t.sunken,
    backgroundColor: t.modified,
  },
  toggle: {
    width: 44,
    height: 26,
    borderRadius: 13,
    padding: 2,
    ...surface.float,
    justifyContent: 'center',
  },
  toggleOn: { backgroundColor: t.accent, borderColor: t.accent, alignItems: 'flex-end' },
  choice: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 11,
    paddingVertical: 9,
    paddingHorizontal: 10,
    borderRadius: size.radiusRow,
  },
  choiceMark: {
    width: 18,
    height: 18,
    marginTop: 1,
    borderWidth: 1.5,
    borderColor: t.dim,
    alignItems: 'center',
    justifyContent: 'center',
  },
  choiceDot: { width: 8, height: 8, borderRadius: 4, backgroundColor: t.accent },
  choiceLabel: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  knob: { width: 20, height: 20, borderRadius: 10, backgroundColor: t.dim },
  knobOn: { backgroundColor: t.onAccent },
});
