// The worktree list while it fills: two projects' headings on the desk, each
// over a ruled card of a few rows, drawn as blocks where the badge, the tile,
// the name and the line under it will be, on the list's own insets so
// nothing moves when the real rows land. A slow pulse runs down the rows, top to bottom; under
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

import { ROW_INSET, RuledCard, TILE, TILE_GAP } from './ui';
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

/** Where each project's lines start in the pulse, its own and then its
 * rows'. Worked out here rather than counted while drawing, which the React
 * Compiler can't follow. */
const FIRST_LINE = PROJECTS.map((_, p) => PROJECTS.slice(0, p).reduce((lines, q) => lines + 1 + q.rows.length, 0));

export function ListSkeleton() {
  const still = useReducedMotion();
  const clock = useSharedValue(0);
  useEffect(() => {
    if (still) return;
    clock.set(0);
    clock.set(withRepeat(withTiming(1, { duration: PULSE_MS, easing: Easing.linear }), -1, false));
    return () => cancelAnimation(clock);
  }, [still, clock]);

  return (
    <View accessible accessibilityRole="progressbar" accessibilityLabel="Loading worktrees" style={styles.list}>
      {PROJECTS.map((project, p) => (
        <View key={p}>
          <Pulse clock={clock} still={still} index={FIRST_LINE[p]} style={styles.project}>
            <View style={styles.badge} />
            <View style={[styles.bar, styles.name, { width: project.name }]} />
          </Pulse>
          <RuledCard style={styles.card}>
            {project.rows.map((row, r) => (
              <Pulse key={r} clock={clock} still={still} index={FIRST_LINE[p] + 1 + r} style={styles.row}>
                <View style={styles.tile} />
                <View style={styles.text}>
                  <View style={[styles.bar, styles.name, { width: row.title }]} />
                  <View style={[styles.bar, styles.detail, { width: row.detail }]} />
                </View>
              </Pulse>
            ))}
          </RuledCard>
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
  // Each project 18 under the one above, as the list spaces them.
  list: { gap: 18 },
  // The project heading's: 56 tall on the desk, in 4 from the card's edge.
  project: { height: 36, flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 4 },
  badge: { width: 20, height: 20, borderRadius: size.radiusSm, backgroundColor: t.selection },
  card: { marginTop: 6 },
  // A ruled row's: its height, insets and columns.
  row: {
    minHeight: size.row,
    flexDirection: 'row',
    alignItems: 'center',
    gap: TILE_GAP,
    paddingVertical: 9,
    paddingHorizontal: ROW_INSET,
  },
  tile: { width: TILE, height: TILE, borderRadius: size.radius, backgroundColor: t.selection },
  text: { flex: 1, minWidth: 0, gap: 8 },
  bar: { borderRadius: 4, backgroundColor: t.selection },
  // A name is brighter than the line under it, as the words are.
  name: { height: 10, backgroundColor: t.border },
  detail: { height: 8 },
});
