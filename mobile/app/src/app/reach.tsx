// A desktop's reach code — ket://reach?host=…&at=… — from the QR code in its
// Settings › Devices › Away from home: every address that desktop can be
// reached at, for a phone paired before Tailscale or ket's relay was on. They
// join the pairing this phone already has. Nothing in the code is secret, and
// the desktop's key is still checked on every connection, so a wrong one can
// only fail to connect.

import { router, useLocalSearchParams } from 'expo-router';
import { useEffect, useState } from 'react';
import { StyleSheet, View } from 'react-native';

import { Bar, Button, Prose, Screen } from '../components/ui';
import { addReachable, reachFrom } from '../lib/addresses';
import { dropConnection } from '../lib/connection';
import { getHost, saveHost } from '../lib/store';
import { size, t } from '../lib/theme';

export default function Reach() {
  const { host, at } = useLocalSearchParams<{ host?: string; at?: string }>();
  // Why it could not be used; `null` while it is being.
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const reach = reachFrom(host, at);
      const saved = reach ? await getHost(reach.hostId) : null;
      if (cancelled) return;
      if (!reach) {
        setProblem('This isn’t a code ket can use. Show the code again on the desktop and scan that.');
        return;
      }
      if (!saved) {
        setProblem('This code is for a desktop this phone isn’t paired with. Pair with it first.');
        return;
      }
      await saveHost(addReachable(saved, reach.addresses));
      // The open connection dials the addresses it started with; the next
      // one is built from what was just saved.
      dropConnection(saved.id);
      if (!cancelled) router.replace(`/devices/${saved.id}`);
    })();
    return () => {
      cancelled = true;
    };
  }, [host, at]);

  return (
    <Screen>
      <Bar back title="Ways to reach it" />
      <View style={styles.page}>
        {problem ? (
          <>
            <Prose>{problem}</Prose>
            <Button kind="primary" title="Pair a desktop" onPress={() => router.replace('/pair')} />
          </>
        ) : (
          <Prose color={t.dim}>Adding the desktop’s addresses…</Prose>
        )}
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  page: { padding: size.gutter, gap: 12, alignItems: 'flex-start' },
});
