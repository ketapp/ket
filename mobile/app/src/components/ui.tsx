// The app's controls, held to design/STYLE-GUIDE.md as the desktop's
// crates/ket-ui/src/ui are: cards on a desk, wells set into them, rounded rows
// with no hairlines, a segmented track for a filter, figures as a dim label
// over a mono value, one primary button per surface, banners tinted by their
// signal. Every screen builds from these; none draws its own control.

import { router } from 'expo-router';
import type { ReactNode } from 'react';
import { Platform, Pressable, StyleSheet, Text, View, type StyleProp, type TextStyle, type ViewStyle } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { font, size, t, tint } from '../lib/theme';
import { Back, Check, Chevron, Gauge, Inbox, ListTodo, Sliders, Tree } from './icons';

/** A whole screen: the desk, clear of the status bar. */
export function Screen({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  const insets = useSafeAreaInsets();
  return <View style={[styles.screen, { paddingTop: insets.top }, style]}>{children}</View>;
}

/** The strip along the top, on the desk: back, a title over a detail, actions. */
export function Bar({
  back,
  title,
  detail,
  detailColor,
  lead,
  right,
}: {
  back?: boolean;
  title?: ReactNode;
  detail?: ReactNode;
  detailColor?: string;
  lead?: ReactNode;
  right?: ReactNode;
}) {
  return (
    <View style={styles.bar}>
      {back ? (
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
      {count !== undefined ? <Text style={[styles.count, color ? { color } : null]}>{count}</Text> : null}
    </View>
  );
}

/**
 * A project's heading: its badge and its name. With `onToggle` it folds the
 * rows away; folded, a dot in the attention colour says one of them is
 * waiting on you, so folding never hides a question. No count of its rows:
 * they are right there, and the number beside the fold's arrow was one more
 * thing to read on a line that only needs the name.
 *
 * With `onBacklog`, open backlog notes show as a count a tap opens, as the
 * desktop's sidebar has them; holding the heading opens the backlog however
 * many there are, as the desktop's project menu does.
 */
export function ProjectHeading({
  name,
  color,
  collapsed,
  waiting,
  onToggle,
  backlog = 0,
  onBacklog,
}: {
  name: string;
  color: string;
  collapsed?: boolean;
  waiting?: boolean;
  onToggle?: () => void;
  backlog?: number;
  onBacklog?: () => void;
}) {
  return (
    <Pressable
      onPress={onToggle}
      onLongPress={onBacklog}
      disabled={!onToggle && !onBacklog}
      accessibilityRole="button"
      accessibilityState={{ expanded: !collapsed }}
      accessibilityHint={onBacklog ? 'Hold for its backlog' : undefined}
      style={({ pressed }) => [styles.project, pressed && { backgroundColor: t.selection }]}
    >
      <Badge name={name} color={color} />
      <Text style={styles.projectName} numberOfLines={1}>
        {name}
      </Text>
      {collapsed && waiting ? <Dot color={t.attention} size={6} /> : null}
      {onBacklog && backlog > 0 ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`Backlog, ${backlog} open`}
          onPress={onBacklog}
          hitSlop={6}
          style={({ pressed }) => [styles.readout, styles.backlog, pressed && { backgroundColor: t.selection }]}
        >
          <ListTodo size={13} color={t.dim} weight={2} />
          <Text style={styles.count}>{backlog > 99 ? '99+' : backlog}</Text>
        </Pressable>
      ) : null}
      {onToggle ? (
        <View style={!collapsed && styles.open}>
          <Chevron size={14} color={t.faint} />
        </View>
      ) : null}
    </Pressable>
  );
}

/** A state a row ends in: the word, in its colour, after its dot. */
export interface RowState {
  word: string;
  color: string;
}

/**
 * One row: a rounded fill with no hairline. A mark, a name in mono, a line of
 * prose under it, and the row's state at the end. `marker` is the amber rail
 * of the row in force — the desktop the picker is showing — and the only place
 * the marker colour is spent.
 */
export function Row({
  mark,
  title,
  detail,
  detailColor,
  state,
  meta,
  marker,
  chevron,
  small,
  onPress,
  children,
}: {
  mark?: ReactNode;
  title: string;
  detail?: string;
  detailColor?: string;
  state?: RowState;
  meta?: ReactNode;
  marker?: boolean;
  chevron?: boolean;
  small?: boolean;
  onPress?: () => void;
  children?: ReactNode;
}) {
  return (
    <Pressable
      onPress={onPress}
      disabled={!onPress}
      style={({ pressed }) => [
        styles.row,
        small && { minHeight: size.rowSm },
        marker && { backgroundColor: t.hover },
        pressed && { backgroundColor: t.selection },
      ]}
    >
      {marker ? <View style={styles.marker} /> : null}
      {mark !== undefined ? <View style={styles.mark}>{mark}</View> : null}
      <View style={styles.rowText}>
        <Text style={[styles.name, small && { fontFamily: font.prose }]} numberOfLines={1}>
          {title}
        </Text>
        {detail ? (
          <Text style={[styles.detail, detailColor ? { color: detailColor } : null]} numberOfLines={1}>
            {detail}
          </Text>
        ) : null}
        {children}
      </View>
      {meta}
      {state ? (
        <View style={styles.state}>
          <Dot color={state.color} size={6} />
          <Text style={[styles.stateText, { color: state.color }]}>{state.word}</Text>
        </View>
      ) : null}
      {chevron ? <Chevron size={16} color={t.faint} /> : null}
    </Pressable>
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

/** One answer in a segmented track. */
export interface Segment<K extends string> {
  key: K;
  label: string;
  count?: number;
  /** A mark before the label: an agent's, for a session tab. */
  icon?: ReactNode;
}

/**
 * A segmented track: a sunken well, the chosen segment raised. A filter
 * shares the width; a count rides in the label a step under it.
 */
export function Segmented<K extends string>({
  segments,
  value,
  onChange,
  style,
}: {
  segments: Segment<K>[];
  value: K;
  onChange: (key: K) => void;
  style?: StyleProp<ViewStyle>;
}) {
  return (
    <View style={[styles.track, style]}>
      {segments.map((segment) => {
        const on = segment.key === value;
        return (
          <Pressable
            key={segment.key}
            accessibilityRole="button"
            accessibilityState={{ selected: on }}
            onPress={() => onChange(segment.key)}
            style={[styles.segment, on && styles.segmentOn]}
          >
            {segment.icon}
            <Text style={[styles.segmentText, on && { color: t.text }]} numberOfLines={1}>
              {segment.label}
            </Text>
            {segment.count !== undefined ? (
              <Text style={[styles.segmentCount, on && { color: t.dim }]}>{segment.count}</Text>
            ) : null}
          </Pressable>
        );
      })}
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
          <Pressable
            key={stat.key}
            accessibilityRole="button"
            accessibilityState={{ selected: on }}
            accessibilityLabel={`${stat.label}, ${stat.count}`}
            onPress={() => onChange(stat.key)}
            style={({ pressed }) => [styles.stat, on && styles.statOn, pressed && !on && { backgroundColor: t.hover }]}
          >
            <Text style={styles.statLabel} numberOfLines={1}>
              {stat.label}
            </Text>
            <View style={styles.statValue}>
              {stat.color ? <Dot color={zero ? t.border : stat.color} size={7} /> : null}
              <Text style={[styles.statNum, zero && { color: t.dim }]}>{stat.count}</Text>
            </View>
          </Pressable>
        );
      })}
    </View>
  );
}

/** A project's badge: its initial in its colour, on its colour at 13%. */
export function Badge({ name, color, size: side = 22 }: { name: string; color: string; size?: number }) {
  return (
    <View style={[styles.badge, { backgroundColor: tint(color, 0.13), width: side, height: side }]}>
      <Text style={[styles.badgeText, { color: lift(color) }]}>{(name.trim()[0] ?? '?').toUpperCase()}</Text>
    </View>
  );
}

/** A project colour made light enough to read on the dark card. */
function lift(hex: string): string {
  const n = parseInt(hex.slice(1), 16);
  const lum = 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255);
  return lum < 90 ? t.dim : hex;
}

/** A quiet tag in a well: a word or a count. */
export function Tag({ children, mono }: { children: ReactNode; mono?: boolean }) {
  return <Text style={[styles.tag, mono && { fontFamily: font.chrome }]}>{children}</Text>;
}

/** A read-out: a figure in a small recessed well, usually led by a glyph. */
export function Readout({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[styles.readout, style]}>{children}</View>;
}

export function Dot({ color, size: side = 7 }: { color: string; size?: number }) {
  return <View style={{ width: side, height: side, borderRadius: side / 2, backgroundColor: color }} />;
}

/** Prose: a sentence, dim unless told otherwise. */
export function Prose({ children, color, style }: { children: ReactNode; color?: string; style?: StyleProp<TextStyle> }) {
  return <Text style={[styles.prose, color ? { color } : null, style]}>{children}</Text>;
}

export function Button({
  title,
  onPress,
  kind = 'secondary',
  icon,
  disabled,
  style,
}: {
  title: string;
  onPress: () => void;
  kind?: 'primary' | 'brand' | 'secondary' | 'ghost' | 'danger';
  icon?: ReactNode;
  disabled?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      style={({ pressed }) => [
        styles.button,
        kind === 'primary' && styles.primary,
        kind === 'brand' && styles.brand,
        kind === 'secondary' && styles.secondary,
        kind === 'danger' && styles.danger,
        pressed &&
          (kind === 'primary'
            ? { backgroundColor: t.accentPressed }
            : kind === 'brand'
              ? { backgroundColor: '#9aeaa4' }
              : { backgroundColor: t.hover }),
        disabled && { opacity: 0.55 },
        style,
      ]}
    >
      {icon}
      <Text
        style={[
          styles.buttonText,
          kind === 'primary' && { color: t.onAccent, fontFamily: font.proseSemibold },
          kind === 'brand' && styles.brandText,
          kind === 'ghost' && { color: t.dim },
          kind === 'danger' && { color: t.failed },
        ]}
      >
        {title}
      </Text>
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
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => [styles.iconButton, well && { backgroundColor: t.sunken }, pressed && { backgroundColor: t.selection }, style]}
    >
      {children}
    </Pressable>
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
    <Pressable
      accessibilityRole={many ? 'checkbox' : 'radio'}
      accessibilityLabel={label}
      accessibilityState={{ checked: on, disabled }}
      onPress={onPress}
      disabled={disabled}
      style={({ pressed }) => [styles.choice, (on || pressed) && { backgroundColor: t.selection }, disabled && { opacity: 0.55 }]}
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
      <View style={styles.rowText}>
        <Text style={styles.choiceLabel}>{label}</Text>
        {description ? <Text style={styles.detail}>{description}</Text> : null}
      </View>
    </Pressable>
  );
}

/** A message tinted by its signal: 8% fill, 25% edge. */
export function Banner({ tone, children, style }: { tone: string; children: ReactNode; style?: StyleProp<ViewStyle> }) {
  return <View style={[styles.banner, { backgroundColor: tint(tone, 0.08), borderColor: tint(tone, 0.25) }, style]}>{children}</View>;
}

/** The three places at the top of the app, as a floating bar at the bottom. */
export function TabBar({ active, needs }: { active: 'worktrees' | 'needs' | 'usage' | 'settings'; needs: number }) {
  const insets = useSafeAreaInsets();
  const tabs = [
    { key: 'worktrees', label: 'Worktrees', path: '/', icon: Tree },
    { key: 'needs', label: 'Needs you', path: '/needs', icon: Inbox },
    { key: 'usage', label: 'Usage', path: '/usage', icon: Gauge },
    { key: 'settings', label: 'Settings', path: '/settings', icon: Sliders },
  ] as const;
  // A capsule of icons, no words; the open tab is a lit green disc.
  return (
    <View style={styles.tabDock}>
      <View style={[styles.tabBar, { marginBottom: Math.max(insets.bottom - 10, 10) }]}>
        {tabs.map(({ key, label, path, icon: Icon }) => {
          const on = key === active;
          return (
            <Pressable
              key={key}
              accessibilityRole="tab"
              accessibilityState={{ selected: on }}
              accessibilityLabel={key === 'needs' && needs > 0 ? `${label}, ${needs}` : label}
              onPress={() => (on ? undefined : router.replace(path))}
              style={[styles.tab, on && styles.tabOn]}
            >
              <View>
                <Icon size={23} color={on ? t.onBrand : t.dim} weight={on ? 1.7 : 1.6} />
                {key === 'needs' && needs > 0 ? (
                  <View style={styles.tabCount}>
                    <Text style={styles.tabCountText}>{needs}</Text>
                  </View>
                ) : null}
              </View>
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}

/** Room a scrolling screen leaves under its content for the tab bar. */
export const TAB_BAR_ROOM = 104;

/** A text field's well, for a `TextInput`. */
export const field: TextStyle = {
  height: size.control,
  borderRadius: size.radius,
  backgroundColor: t.sunken,
  paddingHorizontal: 12,
  color: t.text,
  fontFamily: font.prose,
  fontSize: size.body,
};

export const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: t.backdrop },
  bar: {
    height: size.bar,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: size.gutter,
  },
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
  count: { fontFamily: font.chrome, fontSize: 12, color: t.faint },
  project: {
    height: 40,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 6,
    borderRadius: size.radiusRow,
  },
  open: { transform: [{ rotate: '90deg' }] },
  backlog: { gap: 5, paddingHorizontal: 8 },
  projectName: { flex: 1, fontFamily: font.proseMedium, fontSize: size.name, color: t.text },
  row: {
    minHeight: size.row,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 11,
    paddingVertical: 9,
    paddingLeft: 14,
    paddingRight: 12,
    borderRadius: size.radiusRow,
  },
  marker: { position: 'absolute', left: 0, top: 12, bottom: 12, width: 3, borderRadius: 2, backgroundColor: t.marker },
  mark: { width: 16, alignItems: 'center' },
  rowText: { flex: 1, minWidth: 0, gap: 3 },
  name: { fontFamily: font.chrome, fontSize: size.name, color: t.text },
  detail: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: 11.5 },
  figure: { gap: 3, minWidth: 0 },
  meter: { backgroundColor: t.border, overflow: 'hidden' },
  figureLabel: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  figureValue: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  value: { fontFamily: font.chromeMedium, fontSize: size.value, color: t.text },
  track: { flexDirection: 'row', gap: 2, padding: 3, backgroundColor: t.sunken, borderRadius: size.radius },
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
  segmentOn: { backgroundColor: t.selection },
  segmentText: { fontFamily: font.proseMedium, fontSize: size.label, color: t.dim },
  segmentCount: { fontFamily: font.chrome, fontSize: 11, color: t.faint },
  statsRow: { flexDirection: 'row', gap: 4 },
  stat: { flex: 1, minWidth: 0, gap: 3, paddingVertical: 7, paddingHorizontal: 9, borderRadius: size.radiusRow },
  statOn: { backgroundColor: t.selection },
  statLabel: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  statValue: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  statNum: { fontFamily: font.chromeMedium, fontSize: size.value, color: t.text },
  badge: { borderRadius: size.radiusSm, alignItems: 'center', justifyContent: 'center' },
  badgeText: { fontFamily: font.chromeSemibold, fontSize: 11.5 },
  tag: {
    fontFamily: font.prose,
    fontSize: 11.5,
    color: t.dim,
    backgroundColor: t.sunken,
    paddingHorizontal: 7,
    paddingVertical: 2,
    borderRadius: size.radiusSm,
    overflow: 'hidden',
    alignSelf: 'flex-start',
  },
  readout: {
    height: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 9,
    borderRadius: size.radiusSm,
    backgroundColor: t.sunken,
  },
  prose: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  button: {
    height: size.control,
    borderRadius: size.radius,
    paddingHorizontal: 16,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
  },
  primary: { backgroundColor: t.accent },
  /** The one call to action on a screen that has nothing else yet: green, tall. */
  brand: {
    height: 54,
    gap: 10,
    backgroundColor: t.brand,
    shadowColor: t.brand,
    shadowOpacity: 0.28,
    shadowRadius: 14,
    shadowOffset: { width: 0, height: 8 },
  },
  // The system's own sans, not a brand face: the one call to action reads
  // as the phone's.
  brandText: {
    color: t.onBrand,
    fontFamily: Platform.select({ ios: 'System', default: 'sans-serif' }),
    fontWeight: '600',
    fontSize: 16,
  },
  secondary: { borderWidth: 1, borderColor: t.border },
  danger: { borderWidth: 1, borderColor: tint(t.failed, 0.45) },
  buttonText: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  iconButton: { width: 40, height: 40, borderRadius: size.radius, alignItems: 'center', justifyContent: 'center' },
  banner: { borderWidth: 1, borderRadius: size.radiusCard, padding: 12, gap: 10 },
  toggle: {
    width: 44,
    height: 26,
    borderRadius: 13,
    padding: 2,
    backgroundColor: t.sunken,
    borderWidth: 1,
    borderColor: t.border,
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
  tabDock: { position: 'absolute', left: 0, right: 0, bottom: 0, alignItems: 'center', pointerEvents: 'box-none' },
  tabBar: {
    flexDirection: 'row',
    gap: 14,
    paddingVertical: 6,
    paddingHorizontal: 8,
    backgroundColor: t.card,
    borderWidth: 1,
    borderColor: t.border,
    borderRadius: 34,
    shadowColor: '#000',
    shadowOpacity: 0.6,
    shadowRadius: 18,
    shadowOffset: { width: 0, height: 16 },
  },
  tab: { width: 54, height: 54, borderRadius: 27, alignItems: 'center', justifyContent: 'center' },
  tabOn: {
    backgroundColor: t.brand,
    shadowColor: t.brand,
    shadowOpacity: 0.45,
    shadowRadius: 11,
    shadowOffset: { width: 0, height: 0 },
  },
  tabCount: {
    position: 'absolute',
    top: -8,
    right: -10,
    minWidth: 22,
    height: 22,
    paddingHorizontal: 5,
    borderRadius: 11,
    borderWidth: 2.5,
    borderColor: t.card,
    backgroundColor: t.attention,
    alignItems: 'center',
    justifyContent: 'center',
  },
  tabCountText: { fontFamily: font.chromeSemibold, fontSize: 10.5, color: t.onAccent },
});
