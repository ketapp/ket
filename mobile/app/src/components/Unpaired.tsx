// The whole first screen while no desktop is paired: ket's mark, what the
// phone is for, the three steps on the desktop, and the way into pairing.

import { router } from 'expo-router';
import { useEffect, useState } from 'react';
import { AccessibilityInfo, Animated, Easing, StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import Svg, { Path } from 'react-native-svg';

import { font, t, tint } from '../lib/theme';
import { BrandMark, Camera } from './icons';
import { Button, TAB_BAR_ROOM } from './ui';

const AnimatedPath = Animated.createAnimatedComponent(Path);

/** How many pieces arrive, top to bottom, and the gap between each. */
const PIECES = 9;
const STAGGER = 60;

export function Unpaired() {
  const enter = useEntrance(PIECES);
  return (
    <View style={styles.page}>
      <Rise v={enter[0]} style={styles.top}>
        <BrandMark size={23} />
        <Text style={styles.wordmark}>ket</Text>
      </Rise>

      <View style={styles.middle}>
        <View style={styles.intro}>
          <View accessible accessibilityRole="header" accessibilityLabel="Your agents, in your pocket.">
            <Rise v={enter[1]}>
              <Text style={styles.title}>Your agents,</Text>
            </Rise>
            <Rise v={enter[2]} style={styles.titleLine}>
              <View>
                <Text style={styles.title}>in your pocket</Text>
                <Underline />
              </View>
              <Text style={styles.title}>.</Text>
            </Rise>
          </View>
          <Rise v={enter[3]}>
            <Text style={styles.lede}>Follow every run and answer what it asks.</Text>
          </Rise>
        </View>

        <View style={styles.steps}>
          <Step n={1} v={enter[4]}>
            Open ket on your desktop
          </Step>
          <Step n={2} v={enter[5]}>
            Settings <Text style={{ color: t.faint }}>›</Text> Devices
          </Step>
          <Step n={3} v={enter[6]} last>
            Scan the code it shows
          </Step>
        </View>
      </View>

      <View style={styles.actions}>
        <Rise v={enter[7]} style={styles.half}>
          <Button
            kind="brand"
            title="Scan the code"
            icon={<Camera size={20} color={t.onBrand} weight={1.9} />}
            onPress={() => router.push('/pair')}
          />
        </Rise>
        <Rise v={enter[8]}>
          <Button
            kind="ghost"
            title="Paste a code instead"
            onPress={() => router.push('/pair?paste=1')}
          />
        </Rise>
      </View>
    </View>
  );
}

/**
 * One value per piece, run once on mount: each rises into place a beat after
 * the one above, overshooting a touch. Under Reduce Motion they start in place.
 */
function useEntrance(count: number): Animated.Value[] {
  const [values] = useState(() => Array.from({ length: count }, () => new Animated.Value(0)));
  useEffect(() => {
    let run: Animated.CompositeAnimation | null = null;
    void AccessibilityInfo.isReduceMotionEnabled().then((reduced) => {
      if (reduced) {
        values.forEach((v) => v.setValue(1));
        return;
      }
      run = Animated.stagger(
        STAGGER,
        values.map((v) =>
          Animated.timing(v, { toValue: 1, duration: 420, easing: Easing.out(Easing.back(1.5)), useNativeDriver: true }),
        ),
      );
      run.start();
    });
    return () => run?.stop();
  }, [values]);
  return values;
}

/** A piece of the screen, faded and lifted by its entrance value. */
function Rise({ v, style, children }: { v: Animated.Value; style?: StyleProp<ViewStyle>; children: React.ReactNode }) {
  return (
    <Animated.View
      style={[
        style,
        {
          opacity: v.interpolate({ inputRange: [0, 0.6], outputRange: [0, 1], extrapolate: 'clamp' }),
          transform: [
            { translateY: v.interpolate({ inputRange: [0, 1], outputRange: [22, 0] }) },
            { scale: v.interpolate({ inputRange: [0, 1], outputRange: [0.94, 1] }) },
          ],
        },
      ]}
    >
      {children}
    </Animated.View>
  );
}

/**
 * A step: its number in a ringed disc, joined to the next by a line. On the
 * way in the disc pops and the line grows down toward the next step.
 */
function Step({ n, v, last, children }: { n: number; v: Animated.Value; last?: boolean; children: React.ReactNode }) {
  return (
    <Rise v={v} style={styles.step}>
      {last ? null : (
        <Animated.View
          style={[styles.rule, { transform: [{ scaleY: v.interpolate({ inputRange: [0.5, 1], outputRange: [0, 1], extrapolate: 'clamp' }) }] }]}
        />
      )}
      <Animated.View style={[styles.disc, { transform: [{ scale: v.interpolate({ inputRange: [0, 1], outputRange: [0.3, 1] }) }] }]}>
        <Text style={styles.discText}>{n}</Text>
      </Animated.View>
      <Text style={styles.stepText}>{children}</Text>
    </Rise>
  );
}

/** Two hand-drawn strokes under a phrase, drawn in once, as ketapp.dev's hero does. */
function Underline() {
  const [first] = useState(() => new Animated.Value(0));
  const [second] = useState(() => new Animated.Value(0));

  useEffect(() => {
    let run: Animated.CompositeAnimation | null = null;
    void AccessibilityInfo.isReduceMotionEnabled().then((reduced) => {
      if (reduced) {
        first.setValue(1);
        second.setValue(1);
        return;
      }
      const draw = (v: Animated.Value, delay: number) =>
        Animated.timing(v, { toValue: 1, duration: 560, delay, easing: Easing.bezier(0.6, 0, 0.2, 1), useNativeDriver: false });
      run = Animated.parallel([draw(first, 380), draw(second, 700)]);
      run.start();
    });
    return () => run?.stop();
  }, [first, second]);

  // Dash lengths a little over each path's own length, so a whole dash covers it.
  return (
    <Svg style={styles.underline} viewBox="0 0 400 24" preserveAspectRatio="none" pointerEvents="none">
      <AnimatedPath
        d="M4 16 C 90 6, 210 4, 396 10"
        fill="none"
        stroke={t.brand}
        strokeWidth={4.5}
        strokeLinecap="round"
        strokeDasharray={[420, 420]}
        strokeDashoffset={first.interpolate({ inputRange: [0, 1], outputRange: [420, 0] })}
      />
      <AnimatedPath
        d="M30 20 C 120 12, 220 11, 330 14"
        fill="none"
        stroke={t.brand}
        strokeOpacity={0.55}
        strokeWidth={2.6}
        strokeLinecap="round"
        strokeDasharray={[320, 320]}
        strokeDashoffset={second.interpolate({ inputRange: [0, 1], outputRange: [320, 0] })}
      />
    </Svg>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, paddingHorizontal: 16 },
  top: { height: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10 },
  wordmark: { fontFamily: font.chromeSemibold, fontSize: 23, letterSpacing: -0.46, color: t.text },
  middle: { flex: 1, justifyContent: 'center', gap: 32 },
  intro: { alignItems: 'center', gap: 26, paddingHorizontal: 8 },
  title: {
    fontFamily: font.proseBlack,
    fontSize: 40,
    lineHeight: 42,
    letterSpacing: -1.8,
    color: t.text,
    textAlign: 'center',
  },
  titleLine: { flexDirection: 'row', justifyContent: 'center' },
  underline: { position: 'absolute', left: -4, right: -4, bottom: -11, height: 18 },
  lede: { fontFamily: font.prose, fontSize: 16, lineHeight: 23, color: t.dim, textAlign: 'center' },
  steps: { paddingHorizontal: 20 },
  step: { height: 60, flexDirection: 'row', alignItems: 'center', gap: 16 },
  rule: {
    position: 'absolute',
    left: 15.25,
    top: 49,
    height: 22,
    width: 1.5,
    borderRadius: 1,
    backgroundColor: t.border,
    transformOrigin: 'top',
  },
  disc: {
    width: 32,
    height: 32,
    borderRadius: 16,
    borderWidth: 1.5,
    borderColor: tint(t.brand, 0.55),
    alignItems: 'center',
    justifyContent: 'center',
  },
  discText: { fontFamily: font.chromeMedium, fontSize: 13.5, color: t.brand },
  stepText: { flex: 1, fontFamily: font.prose, fontSize: 15.5, color: t.text },
  actions: { alignItems: 'center', gap: 14, paddingBottom: TAB_BAR_ROOM + 32 },
  half: { width: '50%' },
});
