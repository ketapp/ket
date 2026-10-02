'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// What the home screen shows in place of its worktrees while the phone is
// reaching for its desktop: the ping, and what is happening. Once it has
// tried for a while without an answer, the ping stops and the screen says
// what to check and offers to try again or pair again.

import { router } from 'expo-router';
import { StyleSheet, Text, View } from 'react-native';

import type { HostConnection } from '../lib/connection';
import { font, size, t } from '../lib/theme';
import { checks, hostOf, sentence, STUCK_MS, useWaited } from './ConnectionHelp';
import { Ping } from './Ping';
import { Button, Card } from './ui';

export function Connecting({ conn }: { conn: HostConnection }) {
  const waited = useWaited(conn);
  const stuck = waited >= STUCK_MS;

  if (!stuck) {
    return (
      <View style={styles.centred} accessibilityLiveRegion="polite">
        <Ping stuck={false} />
        <View style={styles.words}>
          <Text style={styles.title}>Connecting to your Mac</Text>
          <Text style={styles.meta}>through {hostOf(conn.host.relay)}</Text>
        </View>
      </View>
    );
  }

  return (
    <View style={styles.stuck} accessibilityLiveRegion="polite">
      <Ping stuck />
      <View style={styles.words}>
        <Text style={styles.title}>No answer yet</Text>
        <Text style={styles.meta}>
          {Math.round(waited / 1000)}s · attempt {Math.max(conn.attempts, 1)} · still trying
        </Text>
        {conn.error ? <Text style={styles.error}>{sentence(conn.error)}</Text> : null}
      </View>
      <Card>
        {checks(conn).map((check, index) => (
          <View key={index} style={[styles.check, index > 0 && styles.checkRuled]}>
            <View style={styles.dot} />
            <Text style={styles.checkText}>{check}</Text>
          </View>
        ))}
      </Card>
      <View style={styles.actions}>
        <Button kind="brand" title="Retry now" onPress={() => conn.retry()} style={styles.action} />
        <Button title="Pair again" onPress={() => router.push('/pair')} style={styles.action} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  centred: { flex: 1, justifyContent: 'center', gap: 26, paddingBottom: 24 },
  stuck: { gap: 14, paddingTop: 6 },
  words: { alignItems: 'center', gap: 6 },
  title: { fontFamily: font.proseSemibold, fontSize: 20, color: t.text },
  meta: { fontFamily: font.chrome, fontSize: 12, color: t.dim },
  error: { fontFamily: font.chrome, fontSize: 12, lineHeight: 18, color: t.text, textAlign: 'center', paddingHorizontal: 12 },
  check: { flexDirection: 'row', gap: 10, alignItems: 'flex-start', paddingVertical: 9, paddingHorizontal: 12 },
  checkRuled: { borderTopWidth: 1, borderTopColor: t.rule },
  dot: { width: 6, height: 6, borderRadius: 3, backgroundColor: t.quotaWarm, marginTop: 7 },
  checkText: { flex: 1, fontFamily: font.prose, fontSize: size.detail, lineHeight: 19, color: t.dim },
  actions: { flexDirection: 'row', gap: 8 },
  // One height for the pair: a brand button is taller than a secondary one
  // on its own, and side by side the two must match.
  action: { flex: 1, height: 48 },
});
