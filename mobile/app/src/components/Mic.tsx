// The mic inside the composer's field: tap to dictate, tap again to stop.
// While it listens it is filled, with bars that move with your voice. See
// lib/voice.ts for the listening.

import { Pressable, StyleSheet, View } from 'react-native';
import Animated, { type SharedValue, useAnimatedStyle } from 'react-native-reanimated';

import { t } from '../lib/theme';
import { Mic } from './icons';

/** The bars' heights at rest and at their loudest, middle ones tallest. */
const BARS = [
  { rest: 3.6, loud: 7 },
  { rest: 5, loud: 14 },
  { rest: 4.2, loud: 10.5 },
  { rest: 3, loud: 6 },
];

export function MicButton({
  listening,
  level,
  onPress,
}: {
  listening: boolean;
  /** How loud the microphone is, 0 to 1, while listening. */
  level: SharedValue<number>;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={listening ? 'Stop dictation' : 'Dictate'}
      accessibilityState={{ selected: listening }}
      hitSlop={4}
      onPress={onPress}
      style={({ pressed }) => [styles.mic, listening && styles.listening, pressed && !listening && { backgroundColor: t.selection }]}
    >
      {listening ? (
        <View style={styles.bars}>
          {BARS.map((bar, at) => (
            <Bar key={at} rest={bar.rest} loud={bar.loud} level={level} />
          ))}
        </View>
      ) : (
        <Mic size={19} color={t.dim} weight={2} />
      )}
    </Pressable>
  );
}

function Bar({ rest, loud, level }: { rest: number; loud: number; level: SharedValue<number> }) {
  const height = useAnimatedStyle(() => ({ height: rest + (loud - rest) * level.get() }));
  return <Animated.View style={[styles.bar, height]} />;
}

const styles = StyleSheet.create({
  mic: { width: 36, height: 36, margin: 4, borderRadius: 18, alignItems: 'center', justifyContent: 'center' },
  // Listening: the one filled thing in the field.
  listening: { backgroundColor: t.accent },
  bars: { flexDirection: 'row', alignItems: 'center', gap: 1.4, height: 18 },
  bar: { width: 2.4, borderRadius: 1.2, backgroundColor: t.card },
});
