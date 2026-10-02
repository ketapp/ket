'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// Worktrees: the desktop sidebar and header strip, on a phone. One sheet —
// masthead, a stat row that is also the filter, what waits on you, every
// worktree by project — under the sidebar's filter, with hairlines between
// its sections instead of a stack of separate cards.

import { Activity, type SubagentSummary, SubagentSummary_State } from '@ket/remote';
import { router, useFocusEffect } from 'expo-router';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Connecting } from '../components/Connecting';
import { Unpaired } from '../components/Unpaired';
import { AgentMark, BrandMark, Branch, Chevron, Desktop, Hand, Plus } from '../components/icons';
import {
  Card,
  Dot,
  IconButton,
  ProjectHeading,
  Prose,
  Row,
  Screen,
  StatFilter,
  TAB_BAR_ROOM,
  TabBar,
} from '../components/ui';
import { type Filter, agentTerminal, detail, detailColor, everyWorktree, filterOf, projectColor, stateOf } from '../lib/activity';
import type { HostConnection } from '../lib/connection';
import { statusLine, useHosts } from '../lib/hosts';
import { getCollapsed, getDesktop, saveCollapsed, saveDesktop } from '../lib/store';
import { font, size, t, tint } from '../lib/theme';

export default function Worktrees() {
  const conns = useHosts();
  const [chosen, setChosen] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
  const [picking, setPicking] = useState(false);

  useFocusEffect(
    useCallback(() => {
      void getDesktop().then(setChosen);
    }, []),
  );

  const conn = conns?.find((c) => c.host.id === chosen) ?? conns?.[0] ?? null;
  const hostId = conn?.host.id ?? null;

  // Folded projects, kept per desktop on the phone. Held with the desktop they
  // were read for, so switching desktops never shows the last one's folds.
  const [folds, setFolds] = useState<{ host: string; projects: string[] } | null>(null);
  useEffect(() => {
    if (hostId) void getCollapsed(hostId).then((projects) => setFolds({ host: hostId, projects }));
  }, [hostId]);
  const collapsed = folds && folds.host === hostId ? folds.projects : [];

  // A worktree that arrives in a folded project unfolds it: new work is
  // what a person looks for, and a folded heading no longer says how much
  // is under it. The first list a desktop sends is only what was there.
  const seen = useRef<{ host: string; ids: Set<string> } | null>(null);
  const snapshot = conn?.snapshot ?? null;
  useEffect(() => {
    if (!hostId || !snapshot || !folds || folds.host !== hostId) return;
    const ids = new Set(everyWorktree(snapshot).map((l) => l.worktree.id));
    const was = seen.current?.host === hostId ? seen.current.ids : null;
    seen.current = { host: hostId, ids };
    if (!was) return;
    const arrived = snapshot.projects
      .filter((project) => project.worktrees.some((w) => !was.has(w.id)))
      .map((project) => project.id || 'elsewhere');
    const unfold = arrived.filter((key) => folds.projects.includes(key));
    if (unfold.length === 0) return;
    const next = folds.projects.filter((key) => !unfold.includes(key));
    setFolds({ host: hostId, projects: next });
    void saveCollapsed(hostId, next);
  }, [hostId, snapshot, folds]);
  const toggle = (project: string) => {
    if (!hostId) return;
    const next = collapsed.includes(project) ? collapsed.filter((p) => p !== project) : [...collapsed, project];
    setFolds({ host: hostId, projects: next });
    void saveCollapsed(hostId, next);
  };
  const waitingEverywhere = (conns ?? []).reduce(
    (n, c) => n + everyWorktree(c.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );

  if (conns !== null && conns.length === 0) {
    return (
      <Screen>
        <Unpaired />
        <TabBar active="worktrees" needs={0} />
      </Screen>
    );
  }

  const located = everyWorktree(conn?.snapshot ?? null);
  const all = located.map((l) => l.worktree);
  const count = (f: Filter) => all.filter((w) => f === 'all' || filterOf(w.activity) === f).length;
  const waiting = located.filter((l) => l.worktree.activity === Activity.BLOCKED);
  const status = statusLine(conn);
  // Reaching for the desktop: its worktrees are not known yet, so the screen
  // shows the reaching instead of an empty list.
  const down = conn !== null && conn.status !== 'connected';

  const choose = (c: HostConnection) => {
    setChosen(c.host.id);
    setPicking(false);
    void saveDesktop(c.host.id);
  };

  const answer = () =>
    waiting.length === 1 && conn
      ? router.push(
          waiting[0].worktree.question?.terminalId
            ? `/session/${conn.host.id}/${waiting[0].worktree.question.terminalId.toString()}`
            : `/worktree/${conn.host.id}/${encodeURIComponent(waiting[0].worktree.id)}`,
        )
      : router.replace('/needs');

  return (
    <Screen>
      <ScrollView contentContainerStyle={[styles.page, down && styles.pageDown]}>
        <Card style={styles.sheet}>
          <View style={styles.mast}>
            <View style={styles.markBox}>
              <BrandMark size={15} />
            </View>
            <View style={styles.headerText}>
              <View style={styles.statusLine}>
                <Dot size={6} color={status.connected ? t.running : conn?.status === 'offline' ? t.failed : t.faint} />
                <Text style={styles.statusText} numberOfLines={1}>
                  {status.connected ? 'connected · relay' : status.text}
                </Text>
              </View>
              <Text style={styles.desktopName} numberOfLines={1}>
                {conn?.host.name ?? ''}
              </Text>
            </View>
            {conn && status.connected ? (
              <IconButton label="New task" well onPress={() => router.push(`/new/${conn.host.id}`)} style={styles.switch}>
                <Plus size={18} color={t.text} />
              </IconButton>
            ) : null}
            {(conns?.length ?? 0) > 1 ? (
              <IconButton label="Switch desktop" well onPress={() => setPicking(!picking)} style={styles.switch}>
                <Desktop size={17} color={picking ? t.text : t.dim} />
              </IconButton>
            ) : null}
          </View>

          {picking ? (
            <View>
              {(conns ?? []).map((c) => (
                <Row
                  key={c.host.id}
                  small
                  marker={c === conn}
                  mark={<Desktop size={15} />}
                  title={c.host.name}
                  detail={statusLine(c).text}
                  onPress={() => choose(c)}
                />
              ))}
            </View>
          ) : null}

          {down ? null : (
            <>
              <View style={styles.hr} />
              <StatFilter
                stats={[
                  { key: 'all' as Filter, label: 'All', count: count('all') },
                  { key: 'running' as Filter, label: 'Running', count: count('running'), color: t.running },
                  { key: 'waiting' as Filter, label: 'Waiting', count: count('waiting'), color: t.attention },
                  { key: 'idle' as Filter, label: 'Idle', count: count('idle'), color: t.faint },
                ]}
                value={filter}
                onChange={setFilter}
                style={styles.statsPad}
              />

              {waiting.length > 0 && conn ? (
                <>
                  <View style={styles.hr} />
                  <View style={styles.alertWrap}>
                    <Pressable style={({ pressed }) => [styles.alertRow, pressed && { backgroundColor: tint(t.attention, 0.14) }]} onPress={answer}>
                      <Hand size={17} color={t.attention} />
                      <View style={styles.headerText}>
                        <Text style={styles.bannerTitle}>{waiting.length === 1 ? '1 waiting on you' : `${waiting.length} waiting on you`}</Text>
                        <Text style={styles.bannerDetail} numberOfLines={1}>
                          {waiting[0].worktree.name} · {waiting[0].worktree.question?.subject || detail(waiting[0].worktree)}
                        </Text>
                      </View>
                      <View style={styles.answerLink}>
                        <Text style={styles.answerLinkText}>Answer</Text>
                        <Chevron size={13} color={t.attention} />
                      </View>
                    </Pressable>
                  </View>
                </>
              ) : null}

              <View style={styles.hr} />

              <View style={styles.listSection}>
                {(conn?.snapshot?.projects ?? []).map((project) => {
                  const rows = project.worktrees.filter((w) => filter === 'all' || filterOf(w.activity) === filter);
                  // Work this phone just asked for, until its worktree is here.
                  const starting =
                    filter === 'all' || filter === 'running'
                      ? (conn?.starting ?? []).filter((work) => work.project === project.id)
                      : [];
                  // All lists every project, as the desktop's sidebar does —
                  // a desktop from 1.9 sends them running or not. The other
                  // filters list only projects with a row to show.
                  if (rows.length === 0 && starting.length === 0 && filter !== 'all') return null;
                  // A desktop from 1.9 keeps its backlogs.
                  const keeps = conn !== null && conn.hostMinor >= 9 && project.id !== '';
                  const key = project.id || 'elsewhere';
                  const folded = collapsed.includes(key);
                  return (
                    <View key={key}>
                      <ProjectHeading
                        name={project.name || 'Elsewhere'}
                        color={projectColor(project)}
                        collapsed={folded}
                        waiting={rows.some((w) => w.activity === Activity.BLOCKED)}
                        onToggle={() => toggle(key)}
                        backlog={project.backlog}
                        onBacklog={
                          keeps && conn
                            ? () => router.push(`/backlog/${conn.host.id}/${encodeURIComponent(project.id)}`)
                            : undefined
                        }
                      />
                      {starting.map((work) => (
                        <Row
                          key={`starting-${work.at}`}
                          mark={<Branch size={15} color={t.faint} />}
                          title={firstLine(work.prompt)}
                          detail={`Starting on ${conn?.host.name || 'your desktop'}\u2026`}
                          state={{ word: 'starting', color: t.faint }}
                        />
                      ))}
                      {!folded && rows.length === 0 && starting.length === 0 ? (
                        <Prose style={styles.idle}>Nothing running</Prose>
                      ) : null}
                      {folded ? null : rows.map((worktree) => (
                        <Row
                          key={worktree.id}
                          mark={worktree.agent ? <AgentMark agent={worktree.agent} size={16} /> : <Branch size={15} color={t.faint} />}
                          title={worktree.name || 'Other terminals'}
                          detail={detail(worktree)}
                          detailColor={detailColor(worktree.activity)}
                          state={stateOf(worktree.activity)}
                          meta={
                            worktree.linesAdded + worktree.linesRemoved > 0 ? (
                              <Text style={styles.lines}>
                                <Text style={{ color: t.added }}>+{worktree.linesAdded}</Text>{' '}
                                <Text style={{ color: t.removed }}>−{worktree.linesRemoved}</Text>
                              </Text>
                            ) : undefined
                          }
                          onPress={() => {
                            // Straight into the work: the agent's conversation, or
                            // with no agent, the first terminal still open.
                            if (!conn) return;
                            const agentTerm = agentTerminal(worktree);
                            if (agentTerm) {
                              router.push(`/session/${conn.host.id}/${agentTerm.id.toString()}`);
                              return;
                            }
                            const terminal = worktree.terminals.find((x) => !x.closed) ?? worktree.terminals[0];
                            if (terminal) router.push(`/terminal/${conn.host.id}/${terminal.id.toString()}`);
                          }}
                        >
                          {worktree.subagents.length > 0 ? <Subagents list={worktree.subagents} /> : null}
                        </Row>
                      ))}
                    </View>
                  );
                })}
                {conn?.status === 'connected' && (conn.snapshot?.projects.length ?? 0) === 0 ? (
                  <Prose style={styles.note}>Nothing is running on this desktop.</Prose>
                ) : null}
                {all.length > 0 && count(filter) === 0 ? <Prose style={styles.note}>No worktree is {filter}.</Prose> : null}
              </View>
            </>
          )}
        </Card>

        {conn && down ? <Connecting conn={conn} /> : null}
      </ScrollView>
      <TabBar active="worktrees" needs={waitingEverywhere} />
    </Screen>
  );
}

/** Most subagents listed under a row; the rest are counted. */
const MOST_SUBAGENTS = 4;

/** The subagents a worktree's agent has running, under its name as ket's
 * sidebar draws them: an elbow each, what it was sent to do, its state. */
/** The first line of what an agent was asked, to name the row standing in
 * for its worktree. */
function firstLine(prompt: string): string {
  const line = prompt.trim().split('\n')[0] ?? '';
  return line.length > 72 ? `${line.slice(0, 71)}\u2026` : line;
}

function Subagents({ list }: { list: SubagentSummary[] }) {
  const shown = list.slice(0, MOST_SUBAGENTS);
  return (
    <View style={styles.subagents}>
      {shown.map((subagent) => {
        const waiting = subagent.state === SubagentSummary_State.BLOCKED;
        const idle = subagent.state === SubagentSummary_State.IDLE;
        const color = waiting ? t.attention : idle ? t.faint : t.running;
        return (
          <View key={subagent.id} style={styles.subagent}>
            <View style={styles.elbow} />
            <Dot color={color} size={5} />
            <Text style={styles.subagentText} numberOfLines={1}>
              {subagent.label || 'Subagent'}
            </Text>
            {waiting ? <Text style={[styles.subagentState, { color }]}>waiting</Text> : null}
          </View>
        );
      })}
      {list.length > shown.length ? (
        <Text style={styles.subagentMore}>+{list.length - shown.length} more</Text>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  subagents: { gap: 3, paddingTop: 2 },
  subagent: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 18 },
  elbow: {
    width: 9,
    height: 9,
    marginTop: -8,
    borderLeftWidth: 1,
    borderBottomWidth: 1,
    borderBottomLeftRadius: 4,
    borderColor: t.border,
  },
  subagentText: { flex: 1, minWidth: 0, fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  subagentState: { fontFamily: font.chrome, fontSize: size.caption },
  subagentMore: { paddingLeft: 15, fontFamily: font.chrome, fontSize: size.caption, color: t.faint },
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: TAB_BAR_ROOM },
  pageDown: { flexGrow: 1 },
  sheet: {},
  mast: { flexDirection: 'row', alignItems: 'center', gap: 12, paddingVertical: 13, paddingLeft: 14, paddingRight: 12 },
  markBox: {
    width: 34,
    height: 34,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t.border,
    alignItems: 'center',
    justifyContent: 'center',
  },
  headerText: { flex: 1, minWidth: 0, gap: 2 },
  statusLine: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  statusText: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  desktopName: { fontFamily: font.proseSemibold, fontSize: size.value, color: t.text },
  switch: { width: 34, height: 34 },
  hr: { height: 1, backgroundColor: t.rule, marginHorizontal: 12 },
  statsPad: { paddingVertical: 9, paddingHorizontal: 9 },
  alertWrap: { paddingHorizontal: 12, paddingVertical: 10 },
  alertRow: { borderRadius: 9, padding: 11, flexDirection: 'row', alignItems: 'center', gap: 11, backgroundColor: tint(t.attention, 0.08) },
  bannerTitle: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  bannerDetail: { fontFamily: font.chrome, fontSize: 12, color: t.dim },
  answerLink: { flexDirection: 'row', alignItems: 'center', gap: 2 },
  answerLinkText: { fontFamily: font.proseMedium, fontSize: size.label, color: t.attention },
  listSection: { paddingTop: 6, paddingHorizontal: 8, paddingBottom: 8 },
  lines: { fontFamily: font.chrome, fontSize: 12, marginLeft: 8 },
  note: { padding: 8 },
  // Under a project's heading, in from the edge its rows start at.
  idle: { fontSize: size.detail, paddingLeft: 14, paddingBottom: 8 },
});
