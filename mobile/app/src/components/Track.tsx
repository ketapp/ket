// A hairline that says something is moving: a two-point line, with a band of
// colour sweeping along it while it is `live`, filled once `done`, bare
// while still to come. Under a row standing in for new work, and under the
// bar of the chat that stands in for it — one segment per step.

import { useEffect, useState } from 'react';
import { type StyleProp, StyleSheet, View, type ViewStyle } from 'react-native';
import Animated, { Easing, cancelAnimation, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import type { Segment } from '../lib/starting';
import { t } from '../lib/theme';

/** The sweep's length, as a share of the track. */
const BAND = 0.45;
/** One pass, end to end. */
const SWEEP_MS = 1400;

export function Track({ state, color = t.brand, style }: { state: Segment; color?: string; style?: StyleProp<ViewStyle> }) {
  const [width, setWidth] = useState(0);
  const along = useSharedValue(0);
  const live = state === 'live';
  useEffect(() => {
    if (!live) return;
    along.set(0);
    along.set(withRepeat(withTiming(1, { duration: SWEEP_MS, easing: Easing.inOut(Easing.ease) }), -1, false));
    return () => cancelAnimation(along);
  }, [live, along]);
  const band = width * BAND;
  const sweep = useAnimatedStyle(() => ({ transform: [{ translateX: -band + along.get() * (width + band) }] }));
  return (
    <View
      accessible={false}
      onLayout={(e) => setWidth(e.nativeEvent.layout.width)}
      style={[styles.track, state === 'done' && { backgroundColor: color }, style]}
    >
      {live && width > 0 ? (
        <Animated.View style={[styles.band, { width: band }, sweep]}>
          <Svg width="100%" height="100%">
            <Defs>
              <LinearGradient id="track-sweep" x1="0" y1="0" x2="1" y2="0">
                <Stop offset="0" stopColor={color} stopOpacity={0} />
                <Stop offset="0.5" stopColor={color} stopOpacity={1} />
                <Stop offset="1" stopColor={color} stopOpacity={0} />
              </LinearGradient>
            </Defs>
            <Rect width="100%" height="100%" fill="url(#track-sweep)" />
          </Svg>
        </Animated.View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  track: { flex: 1, height: 2, borderRadius: 1, backgroundColor: t.rule, overflow: 'hidden' },
  band: { position: 'absolute', top: 0, bottom: 0, left: 0 },
});
