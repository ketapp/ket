// A permission prompt's risk read, from the Mac's Jev beta: "Low risk · 91%",
// in the level's colour, beside the prompt's title. It informs; the buttons
// under it are still the person's to press.

import type { Risk } from '@ket/remote';
import { StyleSheet, Text, View } from 'react-native';

import { font, t, tint } from '../lib/theme';

const COLOUR: Record<string, string> = { low: t.running, medium: t.quotaWarm, high: t.failed };

export function RiskBadge({ risk }: { risk?: Risk }) {
  const colour = risk ? COLOUR[risk.level] : undefined;
  if (!risk || !colour) return null;
  const level = risk.level.charAt(0).toUpperCase() + risk.level.slice(1);
  return (
    <View
      style={[styles.badge, { backgroundColor: tint(colour, 0.12) }]}
      accessibilityLabel={`${level} risk, ${Math.round(risk.confidence * 100)} percent sure. Beta.`}
    >
      <Text style={[styles.level, { color: colour }]}>
        {level} risk · {Math.round(risk.confidence * 100)}%
      </Text>
      <Text style={styles.beta}>beta</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  badge: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    alignSelf: 'flex-start',
    height: 22,
    borderRadius: 11,
    paddingHorizontal: 8,
  },
  level: { fontFamily: font.chromeMedium, fontSize: 11.5 },
  beta: { fontFamily: font.chrome, fontSize: 10, color: t.faint },
});
