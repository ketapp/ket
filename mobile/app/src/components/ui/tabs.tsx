// Tabs: a strip of them scrolling sideways, and the bar of the app's own
// tabs at the foot of the screen.

import { router } from 'expo-router';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import {
  Pressable,
  ScrollView,
  type ScrollViewInstance,
  StyleSheet,
  Text,
  TouchableOpacity,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { showBeside, useSplit } from '../../lib/layout';
import { font, size, t } from '../../lib/theme';
import { PRESSED, surface } from './surface';
import { Gauge, Inbox, Sliders, Tree } from '../icons';
import { shared } from './shared';

/** One tab in a {@link TabStrip}. */
export interface StripTab<K extends string> {
  key: K;
  label: string;
  /** The tab's mark: an agent's, or a terminal's glyph. */
  icon?: ReactNode;
  /** What the tab's agent is doing, as a dot on its mark. */
  state?: { word: string; color: string };
}

/** How far from the strip's ends a tab is scrolled to, clear of the fade. */
const STRIP_FADE = 28;

/**
 * A strip of tabs: the segmented track, but each tab hugs its label and the
 * strip scrolls sideways when they outgrow it, fading at whichever end has
 * more past it. The chosen tab is scrolled into view — at once when the
 * strip first draws, gliding after — and the agent's state rides on its
 * mark, as the desktop's tab strip draws it.
 */
export function TabStrip<K extends string>({
  tabs,
  value,
  onChange,
  label,
  style,
}: {
  tabs: StripTab<K>[];
  value: K;
  onChange: (key: K) => void;
  /** What the strip holds, for a screen reader: "Terminals in main". */
  label: string;
  style?: StyleProp<ViewStyle>;
}) {
  const scroller = useRef<ScrollViewInstance>(null);
  // Where each tab sits in the scrolled row, and how much of the row shows.
  const spans = useRef(new Map<K, { x: number; width: number }>());
  const shown = useRef({ offset: 0, width: 0, content: 0 });
  // The tab last scrolled to: each choice is brought into view once, so the
  // strip stays where a thumb left it while titles change under it.
  const revealed = useRef<K | null>(null);
  const [edges, setEdges] = useState({ start: false, end: false });

  const measure = () => {
    const { offset, width, content } = shown.current;
    const start = offset > 1;
    const end = offset + width < content - 1;
    setEdges((was) => (was.start === start && was.end === end ? was : { start, end }));
  };

  const reveal = () => {
    const span = spans.current.get(value);
    const { offset, width } = shown.current;
    if (!span || !width || revealed.current === value) return;
    const animated = revealed.current !== null;
    revealed.current = value;
    const from = span.x - STRIP_FADE;
    const to = span.x + span.width + STRIP_FADE - width;
    if (offset > from) scroller.current?.scrollTo({ x: Math.max(0, from), animated });
    else if (offset < to) scroller.current?.scrollTo({ x: to, animated });
  };

  // A choice made elsewhere — the screen switching terminals — is revealed
  // here; one made before the tabs were laid out waits for their layout.
  useEffect(reveal);

  return (
    <View style={[styles.strip, style]} accessibilityRole="tablist" accessibilityLabel={label}>
      <ScrollView
        ref={scroller}
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.stripRow}
        scrollEventThrottle={32}
        onScroll={(e) => {
          shown.current.offset = e.nativeEvent.contentOffset.x;
          measure();
        }}
        onLayout={(e) => {
          shown.current.width = e.nativeEvent.layout.width;
          measure();
          reveal();
        }}
        onContentSizeChange={(width) => {
          shown.current.content = width;
          measure();
        }}
      >
        {tabs.map((tab) => {
          const on = tab.key === value;
          return (
            <TouchableOpacity
              key={tab.key}
              accessibilityRole="tab"
              accessibilityLabel={tab.state ? `${tab.label}, ${tab.state.word}` : tab.label}
              accessibilityState={{ selected: on }}
              hitSlop={{ top: 3, bottom: 3 }}
              onPress={() => onChange(tab.key)}
              onLayout={(e) => {
                spans.current.set(tab.key, { x: e.nativeEvent.layout.x, width: e.nativeEvent.layout.width });
                if (on) reveal();
              }}
              activeOpacity={on ? 1 : PRESSED}
              style={[styles.stripTab, on && shared.segmentOn]}
            >
              {tab.icon ? (
                <Marked dot={tab.state?.color} ring={on ? t.selection : t.sunken}>
                  {tab.icon}
                </Marked>
              ) : null}
              <Text style={[styles.stripLabel, on && { color: t.text }]} numberOfLines={1}>
                {tab.label}
              </Text>
            </TouchableOpacity>
          );
        })}
      </ScrollView>
      {edges.start ? <StripFade side="start" /> : null}
      {edges.end ? <StripFade side="end" /> : null}
    </View>
  );
}

/** A strip's end, faded into its well while there is more past it. */
function StripFade({ side }: { side: 'start' | 'end' }) {
  const id = `strip-fade-${side}`;
  return (
    <View pointerEvents="none" style={[styles.stripFade, side === 'start' ? { left: 0 } : { right: 0 }]}>
      <Svg width="100%" height="100%">
        <Defs>
          <LinearGradient id={id} x1={side === 'start' ? 1 : 0} y1={0} x2={side === 'start' ? 0 : 1} y2={0}>
            <Stop offset={0} stopColor={t.sunken} stopOpacity={0} />
            <Stop offset={1} stopColor={t.sunken} stopOpacity={1} />
          </LinearGradient>
        </Defs>
        <Rect width="100%" height="100%" fill={`url(#${id})`} />
      </Svg>
    </View>
  );
}

/** A mark with a state's dot on its corner, ringed in the colour it sits
 * on so the two read apart. No dot, just the mark. */
export function Marked({ children, dot, ring }: { children: ReactNode; dot?: string; ring: string }) {
  return (
    <View>
      {children}
      {dot ? <View style={[styles.markDot, { backgroundColor: dot, borderColor: ring }]} /> : null}
    </View>
  );
}

/** The places at the top of the app, as a floating bar at the bottom. On a
 * wide screen it floats at the foot of the sidebar instead, and the screens
 * beside it leave theirs out. */
export function TabBar({
  active,
  needs,
  sidebar,
}: {
  active: 'worktrees' | 'needs' | 'usage' | 'settings';
  needs: number;
  sidebar?: boolean;
}) {
  const insets = useSafeAreaInsets();
  const split = useSplit();
  if (split && !sidebar) return null;
  const tabs = [
    { key: 'worktrees', label: 'Worktrees', path: '/', icon: Tree },
    { key: 'usage', label: 'Usage', path: '/usage', icon: Gauge },
    { key: 'needs', label: 'Needs you', path: '/needs', icon: Inbox },
    { key: 'settings', label: 'Settings', path: '/settings', icon: Sliders },
  ] as const;
  // A float of icons, no words; the open tab is raised, as the desktop's
  // segmented track raises its chosen segment.
  return (
    <View style={styles.tabDock}>
      <View style={[styles.tabBar, { marginBottom: tabBarFoot(insets.bottom) }]}>
        {tabs.map(({ key, label, path, icon: Icon }) => {
          const on = key === active;
          return (
            <Pressable
              key={key}
              accessibilityRole="tab"
              accessibilityState={{ selected: on }}
              accessibilityLabel={key === 'needs' && needs > 0 ? `${label}, ${needs}` : label}
              onPress={() => {
                if (on) return;
                if (split) {
                  showBeside(path);
                  return;
                }
                // Usage sits on the stack over Worktrees, so a back gesture
                // returns to the list. The other tabs still replace in place.
                if (key === 'usage') {
                  router.push(path);
                  return;
                }
                // From Usage, down to the tabs under it and on to this one.
                // Replacing Usage put a second set of tabs on the stack over
                // the first, which stayed mounted under it, its Worktrees
                // still drawing every snapshot; another set for each visit.
                // Worktrees too: going back alone showed whichever tab Usage
                // was opened from, so Usage opened from Needs you went back
                // to Needs you.
                if (active === 'usage') {
                  router.dismissTo(path);
                  return;
                }
                router.replace(path);
              }}
              style={[styles.tab, on && styles.tabOn]}
            >
              <View>
                <Icon size={21} color={on ? t.text : t.dim} weight={on ? 1.7 : 1.6} />
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

/** The tab bar's height, edge to edge. */
export const TAB_BAR_HEIGHT = 58;

/** How far the tab bar floats above the screen's foot, given the safe area's
 * inset there — what anything floating over it stands on. */
export function tabBarFoot(insetBottom: number): number {
  return Math.max(insetBottom - 10, 10);
}

const styles = StyleSheet.create({
  strip: { ...surface.well, borderRadius: size.radius, overflow: 'hidden' },
  stripRow: { padding: 3, gap: 2 },
  stripTab: {
    height: 34,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 7,
    paddingHorizontal: 11,
    borderRadius: size.radiusSm,
  },
  stripLabel: { maxWidth: 150, fontFamily: font.proseMedium, fontSize: size.label, color: t.dim },
  stripFade: { position: 'absolute', top: 0, bottom: 0, width: STRIP_FADE },
  markDot: { position: 'absolute', right: -5, bottom: -5, width: 11, height: 11, borderRadius: 5.5, borderWidth: 2 },
  tabDock: { position: 'absolute', left: 0, right: 0, bottom: 0, alignItems: 'center', pointerEvents: 'box-none' },
  tabBar: {
    height: TAB_BAR_HEIGHT,
    flexDirection: 'row',
    gap: 2,
    padding: 5,
    ...surface.float,
    borderRadius: 29,
  },
  tab: { width: 64, height: 46, borderRadius: 23, alignItems: 'center', justifyContent: 'center' },
  tabOn: { ...surface.chosen },
  tabCount: {
    position: 'absolute',
    top: -8,
    right: -10,
    minWidth: 22,
    height: 22,
    paddingHorizontal: 5,
    borderRadius: 11,
    borderWidth: 2.5,
    borderColor: t.elevated,
    backgroundColor: t.attention,
    alignItems: 'center',
    justifyContent: 'center',
  },
  tabCountText: { fontFamily: font.chromeSemibold, fontSize: 10.5, color: t.onAccent },
});
