'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// What a screen shows when the phone has been trying to reach its desktop
// for a while: what went wrong, the few things that cause it, and what to do.
// Before that, a plain "Connecting…" — most connections take a second, and a
// checklist flashed at every one of them would teach people to ignore it.

import { router } from 'expo-router';
import { type ReactNode, useEffect, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import type { HostConnection } from '../lib/connection';
import { font, size, t } from '../lib/theme';
import { Banner, Button, Prose } from './ui';

/** How long a connection may try quietly before the phone says more. */
export const STUCK_MS = 8_000;

/** How long `conn` has been down, in milliseconds — 0 while it is up —
 * redrawn each second while down, so what depends on it appears on time and
 * the wait it reports keeps counting. */
export function useWaited(conn: HostConnection): number {
  const [now, setNow] = useState(() => Date.now());
  const down = conn.status !== 'connected';
  useEffect(() => {
    if (!down) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [down]);
  if (!down || conn.downSince === null) return 0;
  return now - conn.downSince;
}

/** The things that stop a phone reaching its desktop, most likely first. */
export function checks(conn: HostConnection): ReactNode[] {
  return [
    'This phone is on the same Wi-Fi as the Mac — not mobile data.',
    'ket is open on the Mac, and Settings › Devices is turned on.',
    'The Mac is awake. A closed lid or sleep drops every phone.',
    <>
      Safari can reach <Text style={styles.address}>{hostOf(conn.host.relay)}</Text> — if the Mac changed network since
      pairing, pair again.
    </>,
  ];
}

export function ConnectionHelp({ conn }: { conn: HostConnection }) {
  const waited = useWaited(conn);
  if (conn.status === 'connected') return null;
  if (waited < STUCK_MS) {
    return <Prose style={styles.quiet}>Connecting to {conn.host.name}…</Prose>;
  }

  return (
    <Banner tone={conn.status === 'offline' ? t.failed : t.attention}>
      <View style={styles.head}>
        <Text style={styles.title}>{`Can’t reach ${conn.host.name}`}</Text>
        <Text style={styles.meta}>
          {Math.round(waited / 1000)}s · attempt {Math.max(conn.attempts, 1)}
        </Text>
      </View>
      {conn.error ? <Text style={styles.error}>{sentence(conn.error)}</Text> : null}
      <View style={styles.checks}>
        {checks(conn).map((check, index) => (
          <Check key={index} n={index + 1}>
            {check}
          </Check>
        ))}
      </View>
      <View style={styles.actions}>
        <Button kind="primary" title="Retry now" onPress={() => conn.retry()} style={styles.grow} />
        <Button title="Pair again" onPress={() => router.push('/pair')} style={styles.grow} />
      </View>
    </Banner>
  );
}

function Check({ n, children }: { n: number; children: React.ReactNode }) {
  return (
    <View style={styles.check}>
      <Text style={styles.num}>{n}</Text>
      <Text style={styles.checkText}>{children}</Text>
    </View>
  );
}

export function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1) + (/[.!?]$/.test(text) ? '' : '.');
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

const styles = StyleSheet.create({
  quiet: { padding: 8 },
  head: { flexDirection: 'row', alignItems: 'baseline', gap: 10 },
  title: { flex: 1, fontFamily: font.proseSemibold, fontSize: size.body, color: t.text },
  meta: { fontFamily: font.chrome, fontSize: 11.5, color: t.dim },
  error: { fontFamily: font.chrome, fontSize: 12.5, lineHeight: 18, color: t.text },
  checks: { gap: 8 },
  check: { flexDirection: 'row', gap: 10, alignItems: 'flex-start' },
  num: {
    width: 20,
    height: 20,
    lineHeight: 20,
    borderRadius: 5,
    overflow: 'hidden',
    textAlign: 'center',
    backgroundColor: t.sunken,
    fontFamily: font.chrome,
    fontSize: 11,
    color: t.text,
  },
  checkText: { flex: 1, fontFamily: font.prose, fontSize: size.detail, lineHeight: 19, color: t.dim },
  address: { fontFamily: font.chrome, color: t.text },
  actions: { flexDirection: 'row', gap: 8 },
  grow: { flex: 1 },
});
