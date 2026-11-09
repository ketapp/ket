// The worktree list while it fills: two projects' headings and a few rows,
// drawn as blocks where the badge, the mark, the name, the line under it
// and the state will be, on the list's own insets so nothing moves when the
// real rows land. A slow pulse runs down the rows, top to bottom; under
// Reduce Motion they hold still.

import { type ReactNode, useEffect } from 'react';
import { type DimensionValue, type StyleProp, StyleSheet, View, type ViewStyle } from 'react-native';
import Animated, {
  Easing,
  cancelAnimation,
  useAnimatedStyle,
  useReducedMotion,
  useSharedValue,
  withRepeat,
  withTiming,
  type SharedValue,
} from 'react-native-reanimated';

import { MARK_GAP, MARK_SLOT, ROW_INSET } from './ui';
import { size, t } from '../lib/theme';

/** One pulse, through every row. */
const PULSE_MS = 1600;
/** How far behind the one above each row's pulse runs, as a share of it. */
const STAGGER = 0.09;
/** The dimmest a block gets, and where it rests under Reduce Motion. */
const LOW = 0.4;
const STILL = 0.7;

/** Each project's rows: how wide the name and the line under it run. */
const PROJECTS: { name: number; rows: { title: DimensionValue; detail: DimensionValue }[] }[] = [
  {
    name: 92,
    rows: [
      { title: '44%', detail: '74%' },
      { title: '58%', detail: '62%' },
      { title: '36%', detail: '80%' },
    ],
  },
  {
    name: 70,
    rows: [
      { title: '52%', detail: '68%' },
      { title: '40%', detail: '58%' },
    ],
  },
];

export function ListSkeleton() {
  const still = useReducedMotion();
  const clock = useSharedValue(0);
  useEffect(() => {
    if (still) return;
    clock.set(0);
    clock.set(withRepeat(withTiming(1, { duration: PULSE_MS, easing: Easing.linear }), -1, false));
    return () => cancelAnimation(clock);
  }, [still, clock]);

  let line = 0;
  return (
    <View accessible accessibilityRole="progressbar" accessibilityLabel="Loading worktrees" style={styles.list}>
      {PROJECTS.map((project, p) => (
        <View key={p}>
          <Pulse clock={clock} still={still} index={line++} style={styles.project}>
            <View style={styles.badge} />
            <View style={[styles.bar, styles.name, { width: project.name }]} />
          </Pulse>
          {project.rows.map((row, r) => (
            <Pulse key={r} clock={clock} still={still} index={line++} style={styles.row}>
              <View style={styles.mark}>
                <View style={styles.dot} />
              </View>
              <View style={styles.text}>
                <View style={[styles.bar, styles.name, { width: row.title }]} />
                <View style={[styles.bar, styles.detail, { width: row.detail }]} />
              </View>
              <View style={[styles.bar, styles.state]} />
            </Pulse>
          ))}
        </View>
      ))}
    </View>
  );
}

/** One line of the skeleton, brightening and dimming a beat after the one
 * above it. */
function Pulse({
  clock,
  still,
  index,
  style,
  children,
}: {
  clock: SharedValue<number>;
  still: boolean;
  index: number;
  style: StyleProp<ViewStyle>;
  children: ReactNode;
}) {
  const pulse = useAnimatedStyle(() => {
    if (still) return { opacity: STILL };
    const lift = 0.5 - 0.5 * Math.cos(2 * Math.PI * (clock.get() - index * STAGGER));
    return { opacity: LOW + (1 - LOW) * lift };
  });
  return <Animated.View style={[style, pulse]}>{children}</Animated.View>;
}

const styles = StyleSheet.create({
  list: { paddingTop: 4 },
  // The project heading's: 48 tall, the badge in the marks' column.
  project: { height: 48, flexDirection: 'row', alignItems: 'center', gap: MARK_GAP, paddingLeft: ROW_INSET },
  badge: { width: 22, height: 22, borderRadius: size.radiusSm, backgroundColor: t.selection },
  // A worktree row's: its height, insets and columns.
  row: {
    minHeight: size.row,
    flexDirection: 'row',
    alignItems: 'center',
    gap: MARK_GAP,
    paddingVertical: 9,
    paddingLeft: ROW_INSET,
    paddingRight: 12,
  },
  mark: { width: MARK_SLOT, alignItems: 'center' },
  dot: { width: 16, height: 16, borderRadius: 8, backgroundColor: t.selection },
  text: { flex: 1, minWidth: 0, gap: 8 },
  bar: { borderRadius: 4, backgroundColor: t.selection },
  // A name is brighter than the line under it, as the words are.
  name: { height: 10, backgroundColor: t.border },
  detail: { height: 8 },
  state: { width: 46, height: 8 },
});
