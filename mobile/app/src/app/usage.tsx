// Usage: the desktop's usage popover, whole — every agent, every window of
// its plan, when each resets, how old the reading is — for the desktop the
// Worktrees tab shows, under the same head as Worktrees. Opened over
// Worktrees from the tab bar, so a back gesture returns to the list. The
// desktop reads it; this only draws what it last said.
//
// Each agent is a heading on the desk, its plan and what of it is running
// here beside its name, over a ruled card of its plan's windows, each led by
// a ring of how much of it is used. Over them all, any session its plan has
// stopped, with Switch to hand its task to another agent.

import {
  Activity,
  type ProviderUsage,
  type SessionLimit,
  SessionLimit_Scope,
  SessionLimit_Window,
  type UsageWindow,
  UsageStatus,
} from '@ket/remote';
import { router, useFocusEffect } from 'expo-router';
import { useCallback, useMemo, useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { HostHeader } from '../components/HostHeader';
import {
  AgentTile,
  Card,
  Dot,
  Pill,
  ProjectHeading,
  Prose,
  Ring,
  Row,
  RuledCard,
  Screen,
  SectionLabel,
  TAB_BAR_ROOM,
  TabBar,
} from '../components/ui';
import { everyWorktree } from '../lib/activity';
import type { HostConnection } from '../lib/connection';
import { type PairedHost, useHeldHosts, useHostStates } from '../lib/hosts';
import { useSplit } from '../lib/layout';
import { isOver, limitKey, pickAgent, reopenLimitSheet, stoppedAt } from '../lib/limits';
import { getDesktop, heldDesktop, saveDesktop } from '../lib/store';
import { font, size, t } from '../lib/theme';
import { compactDuration, hasReading, heard, metadata, pressure, resetsIn, sessions, useNow, usedOf } from '../lib/usage';

export default function Usage() {
  // Opened from the tab bar, a new screen each time: the connections already
  // held draw its cards at once, rather than a note saying none is paired
  // until the Keychain gives the list back.
  const conns = useHeldHosts();
  const hostStates = useHostStates(conns);
  const hosts = useMemo<readonly PairedHost[] | null>(
    () => (conns && hostStates ? conns.map((conn, at) => ({ conn, hostState: hostStates[at] })) : null),
    [conns, hostStates],
  );
  // The desktop chosen on the Worktrees tab, else the first; as last read,
  // so the first draw is already that desktop's.
  const [chosen, setChosen] = useState<string | null>(heldDesktop);
  useFocusEffect(
    useCallback(() => {
      void getDesktop().then(setChosen);
    }, []),
  );
  const choose = (conn: HostConnection) => {
    setChosen(conn.id);
    void saveDesktop(conn.id);
  };
  // Beside the sidebar on a wide screen, which already wears this head.
  const split = useSplit();
  const current = hosts?.find((h) => h.conn.id === chosen) ?? hosts?.[0] ?? null;
  const hostState = current?.hostState ?? null;
  const waiting = (hostStates ?? []).reduce(
    (n, h) => n + everyWorktree(h.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );
  const now = useNow();
  const snapshot = hostState?.snapshot ?? null;
  const providers = snapshot?.usage ?? [];
  // Agents with a reading first, in the desktop's order; the rest after.
  const ordered = [...providers.filter(hasReading), ...providers.filter((p) => !hasReading(p))];
  // Sessions their plan has stopped (1.38), until each resets.
  const stops =
    current && (hostState?.hostMinor ?? 0) >= 38
      ? everyWorktree(snapshot).flatMap(({ worktree }) => {
          const stop = stoppedAt(worktree);
          return stop && !isOver(stop.limit, now) ? [{ worktree, ...stop }] : [];
        })
      : [];
  // Switch hands the task over in its chat (1.39), where the limit's sheet
  // offers the agent to take it.
  const canSwitch = (hostState?.hostMinor ?? 0) >= 39;
  // Saying there is nothing waits until that is known: the paired desktops
  // read, and this one heard from (see `heard`).
  const nothing = hostState
    ? heard(hostState)
      ? `No usage yet. It shows once ket is open on ${hostState.host.name} and has read it.`
      : null
    : conns
      ? 'No desktop paired yet. Pair one in Settings.'
      : null;
  return (
    <Screen>
      {split ? null : (
        <HostHeader hosts={hosts} current={current} onChoose={choose} onStart={(c) => router.navigate(`/new/${c.id}`)} />
      )}
      <ScrollView contentContainerStyle={styles.page}>
        {current && stops.length > 0 ? (
          <View>
            <SectionLabel color={t.quotaHot}>Out of usage</SectionLabel>
            <RuledCard style={styles.cardGap}>
              {stops.map(({ worktree, agent, limit, terminal }) => {
                const host = current.conn.id;
                const chat = terminal !== undefined ? `/session/${host}/${terminal.toString()}` : undefined;
                const open = () => router.navigate(chat ?? `/worktree/${host}/${encodeURIComponent(worktree.id)}`);
                return (
                  <Row
                    key={worktree.id}
                    ruled
                    mark={<AgentTile agent={agent} tone={t.quotaHot} label={`${agent}, out of usage`} />}
                    title={worktree.name || 'Other terminals'}
                    detail={limitLine(limit, now)}
                    meta={
                      chat && terminal !== undefined && canSwitch && pickAgent(snapshot, agent) ? (
                        <Pill
                          title="Switch"
                          label="Switch to another agent"
                          tone={t.quotaHot}
                          onPress={() => {
                            reopenLimitSheet(limitKey(host, terminal, limit));
                            open();
                          }}
                        />
                      ) : undefined
                    }
                    onPress={open}
                  />
                );
              })}
            </RuledCard>
          </View>
        ) : null}
        {ordered.map((provider) => (
          <Agent key={provider.provider} provider={provider} live={sessions(snapshot, provider.provider)} now={now} />
        ))}
        {providers.length === 0 && nothing ? (
          <Card style={styles.empty}>
            <Prose>{nothing}</Prose>
          </Card>
        ) : null}
      </ScrollView>
      <TabBar active="usage" needs={waiting} />
    </Screen>
  );
}

/** An agent: its heading, and a ruled card of its plan's windows when it
 * has a reading; without one, the heading says why. */
function Agent({ provider, live, now }: { provider: ProviderUsage; live: number; now: number }) {
  const reading = hasReading(provider) && provider.windows.length > 0;
  const detail = hasReading(provider)
    ? provider.windows.length > 0
      ? metadata(provider, now)
      : 'No windows reported'
    : provider.status === UsageStatus.UNAVAILABLE
      ? provider.reason
        ? `Unavailable — ${provider.reason}`
        : 'Unavailable'
      : 'No usage reporting';
  return (
    <View>
      <ProjectHeading
        name={provider.provider}
        mark={<AgentTile agent={provider.provider} size={20} />}
        detail={detail}
        end={
          live > 0 ? (
            <View style={styles.running}>
              <Dot color={t.brand} size={7} />
              <Text style={styles.runningText}>{live} running</Text>
            </View>
          ) : null
        }
      />
      {reading ? (
        <RuledCard style={styles.cardGap}>
          {provider.windows.map((window) => (
            <Window key={window.name} window={window} now={now} />
          ))}
        </RuledCard>
      ) : null}
    </View>
  );
}

/** One window of a plan: a ring of how much is used, its name over when it
 * resets, and the figure. */
function Window({ window, now }: { window: UsageWindow; now: number }) {
  const used = usedOf(window);
  const ramp = pressure(used);
  const reset = resetsIn(window, now);
  return (
    <Row
      ruled
      mark={<Ring share={used / 100} color={ramp ?? t.dim} label={`${used.toFixed(0)}% used`} />}
      title={window.name}
      detail={reset ? `Resets in ${reset}` : undefined}
      meta={<Text style={[styles.figure, ramp ? { color: ramp } : null]}>{used.toFixed(0)}%</Text>}
    />
  );
}

/** What stopped a session, and when it resets: "Session limit · resets in 1h 5m". */
function limitLine(limit: SessionLimit, now: number): string {
  const what =
    limit.scope === SessionLimit_Scope.MODEL
      ? 'Model limit'
      : limit.window === SessionLimit_Window.WEEKLY
        ? 'Weekly limit'
        : limit.window === SessionLimit_Window.SESSION
          ? 'Session limit'
          : 'Usage limit';
  const left = Number(limit.resetsAtMs) - now;
  return limit.resetsAtMs !== 0n && left > 0 ? `${what} · resets in ${compactDuration(left)}` : what;
}

const styles = StyleSheet.create({
  // Under the head, at the Worktrees tab's edge; each agent 16 under the one
  // above it.
  page: { paddingHorizontal: 16, paddingTop: 18, gap: 16, paddingBottom: TAB_BAR_ROOM },
  // A card under its heading.
  cardGap: { marginTop: 6 },
  running: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  runningText: { fontFamily: font.chromeMedium, fontSize: 12, color: t.dim, fontVariant: ['tabular-nums'] },
  figure: { fontFamily: font.chromeSemibold, fontSize: size.body, color: t.text, fontVariant: ['tabular-nums'] },
  empty: { padding: 14 },
});
