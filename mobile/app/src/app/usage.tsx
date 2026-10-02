'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// Usage, a tab: the desktop's usage popover, whole — every agent, every
// window of its plan, when each resets, how old the reading is — for the
// desktop the Worktrees tab shows. The desktop reads it; this only draws what
// it last said.

import { Activity, type ProviderUsage, type UsageWindow, UsageStatus } from '@ket/remote';
import { useFocusEffect } from 'expo-router';
import { useCallback, useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { AgentMark } from '../components/icons';
import { Card, Meter, Prose, Screen, TAB_BAR_ROOM, TabBar } from '../components/ui';
import { everyWorktree } from '../lib/activity';
import { useHosts } from '../lib/hosts';
import { getDesktop } from '../lib/store';
import { font, size, t } from '../lib/theme';
import { metadata, pressure, resetsIn, sessions, useNow, usedOf } from '../lib/usage';

export default function Usage() {
  const conns = useHosts();
  // The desktop chosen on the Worktrees tab, else the first.
  const [chosen, setChosen] = useState<string | null>(null);
  useFocusEffect(
    useCallback(() => {
      void getDesktop().then(setChosen);
    }, []),
  );
  const conn = conns?.find((c) => c.host.id === chosen) ?? conns?.[0] ?? null;
  const waiting = (conns ?? []).reduce(
    (n, c) => n + everyWorktree(c.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );
  const now = useNow();
  const snapshot = conn?.snapshot ?? null;
  const providers = snapshot?.usage ?? [];
  return (
    <Screen>
      <ScrollView contentContainerStyle={styles.page}>
        {providers.map((provider) => (
          <Provider key={provider.provider} provider={provider} live={sessions(snapshot, provider.provider)} now={now} />
        ))}
        {providers.length === 0 ? (
          <Card style={styles.empty}>
            <Prose>
              {conn
                ? `No usage yet. It shows once ket is open on ${conn.host.name} and has read it.`
                : 'No desktop paired yet. Pair one in Settings.'}
            </Prose>
          </Card>
        ) : (
          <Prose style={styles.footnote} color={t.faint}>
            Plan quota, as ket on your desktop last read it. It reads again every few minutes.
          </Prose>
        )}
      </ScrollView>
      <TabBar active="usage" needs={waiting} />
    </Screen>
  );
}

function Provider({ provider, live, now }: { provider: ProviderUsage; live: number; now: number }) {
  const stale = provider.status === UsageStatus.STALE;
  const quiet = provider.status === UsageStatus.UNSUPPORTED;
  const about = metadata(provider, now);
  let body;
  switch (provider.status) {
    case UsageStatus.FRESH:
    case UsageStatus.STALE:
      body =
        provider.windows.length === 0 ? (
          <Note>No windows reported</Note>
        ) : (
          <View style={styles.windows}>
            {provider.windows.map((window) => (
              <Window key={window.name} window={window} now={now} />
            ))}
          </View>
        );
      break;
    case UsageStatus.UNAVAILABLE:
      body = <Note>{provider.reason ? `Unavailable — ${provider.reason}` : 'Unavailable'}</Note>;
      break;
    default:
      body = <Note>No usage reporting</Note>;
  }
  return (
    <Card style={styles.card}>
      <View style={styles.head}>
        <View style={[styles.mark, quiet && { opacity: 0.45 }]}>
          <AgentMark agent={provider.provider} size={16} />
        </View>
        <View style={styles.headText}>
          <Text style={[styles.name, quiet && { color: t.dim }]}>{provider.provider}</Text>
          {about ? (
            <Text style={[styles.meta, stale && { color: t.attention }]} numberOfLines={1}>
              {about}
            </Text>
          ) : null}
        </View>
      </View>
      {body}
      {live > 0 ? <Note>{live === 1 ? '1 session running here' : `${live} sessions running here`}</Note> : null}
    </Card>
  );
}

/** One window of a plan: its name and when it resets over the figure, then its length. */
function Window({ window, now }: { window: UsageWindow; now: number }) {
  const used = usedOf(window);
  const ramp = pressure(used);
  const reset = resetsIn(window, now);
  return (
    <View style={styles.window}>
      <View style={styles.windowLine}>
        <Text style={styles.windowName} numberOfLines={1}>
          {window.name}
        </Text>
        {reset ? <Text style={styles.reset}>Resets in {reset}</Text> : null}
        <Text style={[styles.figure, ramp ? { color: ramp } : null]}>{used.toFixed(0)}%</Text>
      </View>
      <Meter fraction={used / 100} color={ramp ?? t.accent} />
    </View>
  );
}

function Note({ children }: { children: string }) {
  return <Text style={styles.note}>{children}</Text>;
}

const styles = StyleSheet.create({
  // No heading above it: the tab bar already says where this is. Starts where
  // the Worktrees tab's page does.
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: TAB_BAR_ROOM },
  card: { padding: 14, gap: 12 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingBottom: 12, borderBottomWidth: 1, borderBottomColor: t.rule },
  mark: { width: 20, alignItems: 'center' },
  headText: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontFamily: font.proseSemibold, fontSize: size.body, color: t.text },
  meta: { fontFamily: font.chrome, fontSize: 11.5, color: t.dim },
  windows: { gap: 14 },
  window: { gap: 7 },
  windowLine: { flexDirection: 'row', alignItems: 'baseline', gap: 10 },
  windowName: { flex: 1, fontFamily: font.proseMedium, fontSize: size.label, color: t.text },
  reset: { fontFamily: font.chrome, fontSize: 11.5, color: t.dim },
  figure: { minWidth: 40, textAlign: 'right', fontFamily: font.chromeSemibold, fontSize: size.label, color: t.text },
  note: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  empty: { padding: 14 },
  footnote: { fontSize: size.detail, lineHeight: 18, paddingHorizontal: 6 },
});
