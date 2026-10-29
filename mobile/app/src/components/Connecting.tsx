'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// What the home screen shows in place of its worktrees while the phone is
// reaching for its desktop: the ping, and what is happening. Once it has
// tried for a while without an answer, the ping stops and the screen names
// the Mac it cannot reach, says in one sentence what to check, shows the
// address it is trying, and offers to try again or pair again.

import { router } from 'expo-router';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { HostConnection } from '../lib/connection';
import { font, size, t } from '../lib/theme';
import { hostOf, STUCK_MS, useWaited } from './ConnectionHelp';
import { Ping } from './Ping';
import { Button } from './ui';

export function Connecting({ conn }: { conn: HostConnection }) {
  const waited = useWaited(conn);
  const stuck = waited >= STUCK_MS;

  if (!stuck) {
    return (
      <View style={styles.centred} accessibilityLiveRegion="polite">
        <Ping stuck={false} />
        <View style={styles.words}>
          <Text style={styles.title}>Connecting to your Mac</Text>
          <Text style={styles.meta}>{conn.addressForDisplay() ? `through ${hostOf(conn.addressForDisplay()!)}` : 'trying saved addresses'}</Text>
        </View>
      </View>
    );
  }

  const address = conn.addressForDisplay();
  return (
    <View style={styles.stuck} accessibilityLiveRegion="polite">
      <Ping stuck />
      <View style={styles.words}>
        <Text style={styles.title}>{`Can’t reach ${conn.host.name}`}</Text>
        <Text style={styles.meta}>attempt {Math.max(conn.attempts, 1)}</Text>
        <Text style={styles.sentence}>
          Phone and Mac on the same Wi‑Fi or Tailscale, ket running on the Mac, and the Mac awake with its lid open.
        </Text>
      </View>
      <View style={styles.address}>
        <Text style={styles.addressText} numberOfLines={1}>
          {address ? hostOf(address) : 'No saved address'}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Change the saved address"
          hitSlop={8}
          onPress={() => router.push(`/devices/${conn.host.id}`)}
        >
          <Text style={styles.change}>Change</Text>
        </Pressable>
      </View>
      <View style={styles.actions}>
        <Button kind="brand" title="Retry now" onPress={() => conn.retry()} style={styles.action} />
        <Button title="Pair again" onPress={() => router.push('/pair')} style={styles.action} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  centred: { flex: 1, justifyContent: 'center', gap: 26, paddingBottom: 24 },
  // Wider than the page's gutter: a few lines of centred words and two
  // buttons read better with room at the sides than run to the card edge.
  stuck: { gap: 18, paddingTop: 6, paddingHorizontal: 14 },
  words: { alignItems: 'center', gap: 8, paddingHorizontal: 22 },
  title: { fontFamily: font.proseSemibold, fontSize: 19, color: t.text, textAlign: 'center' },
  meta: { fontFamily: font.chrome, fontSize: 12, color: t.dim },
  sentence: { marginTop: 6, fontFamily: font.prose, fontSize: 13, lineHeight: 19, color: t.dim, textAlign: 'center' },
  address: {
    height: 32,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 10,
    paddingHorizontal: 12,
    borderRadius: size.radius,
    backgroundColor: t.sunken,
  },
  addressText: { flexShrink: 1, fontFamily: font.chrome, fontSize: size.detail, color: t.text },
  change: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.dim },
  actions: { flexDirection: 'row', gap: 8 },
  // One height for the pair: a brand button is taller than a secondary one
  // on its own, and side by side the two must match.
  action: { flex: 1, height: 48 },
});
