'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// A terminal showing Claude Code's agent view: the background sessions it
// holds, each opening as a chat of its own. Agent view's sessions run under
// Claude's own daemon and report nothing to the terminal, so the terminal's
// own chat would be empty; since 1.21 the desktop lists them instead.

import { type AgentView, BackgroundSession_State, type BackgroundSession, type HostSnapshot } from '@ket/remote';
import { router } from 'expo-router';
import { useEffect, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { everyWorktree } from '../lib/activity';
import { useHost } from '../lib/hosts';
import { font, size, t } from '../lib/theme';
import { ChevronDown, Send, TerminalWindow } from './icons';
import { KeyboardLift } from './KeyboardLift';
import { Bar, Button, Card, Dot, IconCluster, Prose, Row, Screen, field } from './ui';

/** Terminal `terminalId` as the snapshot has it: `undefined` while the
 * snapshot doesn't, and its agent view, if it shows one, otherwise. */
export function terminalAt(
  snapshot: HostSnapshot | null,
  terminalId: bigint,
): { closed: boolean; agentView?: AgentView } | undefined {
  for (const { worktree } of everyWorktree(snapshot)) {
    const terminal = worktree.terminals.find((x) => x.id === terminalId);
    if (terminal) return { closed: terminal.closed, agentView: terminal.closed ? undefined : terminal.agentView };
  }
  return undefined;
}

/** A background session's state, as a row and a chat's bar show it. */
export function backgroundState(state: BackgroundSession_State | undefined): { word: string; color: string } | undefined {
  switch (state) {
    case BackgroundSession_State.NEEDS_YOU:
      return { word: 'waiting', color: t.attention };
    case BackgroundSession_State.WORKING:
      return { word: 'running', color: t.running };
    case BackgroundSession_State.DONE:
      return { word: 'done', color: t.dim };
    default:
      return undefined;
  }
}

/** How long ago `at` was, in a few characters — and, for a screen reader,
 * in words. */
function ago(at: bigint, now: number): string {
  const seconds = Math.max(0, Math.round((now - Number(at)) / 1000));
  if (seconds < 60) return 'now';
  if (seconds < 3600) return `${Math.round(seconds / 60)} min`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} h`;
  return `${Math.round(seconds / 86400)} d`;
}

const GROUPS: { state: BackgroundSession_State; title: string; color: string }[] = [
  { state: BackgroundSession_State.NEEDS_YOU, title: 'Needs you', color: t.attention },
  { state: BackgroundSession_State.WORKING, title: 'Working', color: t.running },
  { state: BackgroundSession_State.DONE, title: 'Done', color: t.dim },
];

/** How many finished sessions show before the rest fold away. */
const DONE_SHOWN = 4;

/** A session's name as a title: agent view names them in lower case. */
function titled(name: string): string {
  return name.charAt(0).toUpperCase() + name.slice(1);
}

/** `view` is the agent view as last listed; `left` once the terminal has
 * stopped showing it, which keeps the list rather than swapping a chat in
 * under whatever was opened from it. `reading` while the desktop hasn't
 * read agent view's sessions yet.
 *
 * Grouped as agent view groups them, each group one card in the Needs tab's
 * shape: its dot, name and count heading the card, its sessions under them.
 * A session that needs you, or is working, says
 * what — the permission it wants, the step it's on — in its group's colour;
 * a finished one is its name and when it last did anything. Finished ones
 * fold after a few. At the foot, agent view's own box: a task typed there
 * starts a session on the Mac (1.25). */
export function AgentViewList({
  hostId,
  terminal,
  view,
  reading,
  left,
}: {
  hostId: string;
  terminal: string;
  view: AgentView;
  reading: boolean;
  left: boolean;
}) {
  const conn = useHost(hostId);
  const insets = useSafeAreaInsets();
  // Read on a timer, not each render: the ages only need the minute.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);
  const [allDone, setAllDone] = useState(false);
  const [task, setTask] = useState('');
  const [starting, setStarting] = useState(false);
  const [note, setNote] = useState('');
  const terminalId = BigInt(terminal);
  const located = everyWorktree(conn?.snapshot ?? null).find(({ worktree }) =>
    worktree.terminals.some((x) => x.id === terminalId),
  );
  const place = located ? `${located.project.name || 'Elsewhere'} · ${located.worktree.name}` : 'Claude';
  const canStart = conn !== null && conn.hostMinor >= 25;

  const open = (session: BackgroundSession) =>
    router.push(`/agent-session/${hostId}/${terminal}/${encodeURIComponent(session.session)}`);
  // When it last did anything; a desktop before 1.25 says only when it began.
  const when = (session: BackgroundSession) =>
    ago(session.activeAtMs > 0n ? session.activeAtMs : session.startedAtMs, now);

  const start = async () => {
    const text = task.trim();
    if (!conn || !text || starting) return;
    setStarting(true);
    setNote('');
    try {
      if (!(await conn.takeControl(terminalId))) {
        setNote('Another device is typing in this terminal.');
        return;
      }
      if (await conn.openAgentSession(terminalId, '', text)) setTask('');
      else setNote('Your desktop took control back.');
    } catch (e) {
      setNote(`Not started: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      conn.releaseControl(terminalId);
      setStarting(false);
    }
  };

  const groups = GROUPS.map((group) => ({
    ...group,
    sessions: view.sessions.filter((s) => s.state === group.state),
  })).filter((group) => group.sessions.length > 0);
  return (
    <Screen>
      <Bar
        back
        title="Agent view"
        detail={place}
        right={
          // The chat header's own control, so the two agent view screens match.
          <IconCluster
            actions={[
              {
                label: 'Open the terminal',
                onPress: () => router.push(`/terminal/${hostId}/${terminal}`),
                icon: <TerminalWindow size={18} color={t.text} />,
              },
            ]}
          />
        }
      />
      <ScrollView contentContainerStyle={styles.page}>
        {left ? (
          <Card style={styles.empty}>
            <Prose color={t.dim}>
              This terminal isn’t in agent view now. These are its sessions as it last listed them.
            </Prose>
            <Button
              title="Open the chat"
              onPress={() => router.replace(`/session/${hostId}/${terminal}?chat=1`)}
            />
          </Card>
        ) : null}
        {groups.length === 0 ? (
          <Card style={styles.empty}>
            <Prose color={t.dim}>
              {reading
                ? 'Reading agent view’s sessions on your Mac…'
                : 'Agent view lists no background sessions for this checkout yet. Start one below, or in agent view on your Mac.'}
            </Prose>
          </Card>
        ) : (
          groups.map((group) => {
            const done = group.state === BackgroundSession_State.DONE;
            const folded = done && !allDone && group.sessions.length > DONE_SHOWN + 1;
            const shown = folded ? group.sessions.slice(0, DONE_SHOWN) : group.sessions;
            return (
              <Card key={group.state} style={styles.group}>
                {/* A Heading's metrics, with the group's dot before its name. */}
                <View style={styles.groupTitle}>
                  <View style={styles.groupName}>
                    <Dot color={group.color} />
                    <Text style={[styles.groupNameText, { color: group.color }]}>{group.title}</Text>
                  </View>
                  <Text style={styles.groupCount}>{group.sessions.length}</Text>
                </View>
                {shown.map((session) => (
                  <Row
                    key={session.session}
                    small
                    title={titled(session.name)}
                    detail={done ? undefined : session.doing || undefined}
                    detailColor={group.color}
                    meta={
                      <View accessible accessibilityLabel={`last active ${when(session)} ago`}>
                        <Prose color={t.faint} style={styles.when}>
                          {when(session)}
                        </Prose>
                      </View>
                    }
                    chevron
                    onPress={() => open(session)}
                  />
                ))}
                {folded ? (
                  <Pressable
                    accessibilityRole="button"
                    onPress={() => setAllDone(true)}
                    style={({ pressed }) => [styles.more, pressed && { backgroundColor: t.selection }]}
                  >
                    <Text style={styles.moreText}>Show {group.sessions.length - DONE_SHOWN} more</Text>
                    <ChevronDown size={14} color={t.accent} weight={2.2} />
                  </Pressable>
                ) : null}
              </Card>
            );
          })
        )}
        {note ? (
          <Prose color={t.dim} style={styles.note}>
            {note}
          </Prose>
        ) : null}
      </ScrollView>
      {canStart ? (
        <KeyboardLift offset={{ closed: 0, opened: insets.bottom - 6 }} style={[styles.composer, { paddingBottom: insets.bottom + 6 }]}>
          <TextInput
            value={task}
            onChangeText={setTask}
            placeholder="Describe a task for a new session"
            placeholderTextColor={t.faint}
            accessibilityLabel="A task for a new session"
            returnKeyType="send"
            onSubmitEditing={() => void start()}
            editable={!starting}
            style={[field, styles.task]}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Start the session"
            disabled={!task.trim() || starting}
            onPress={() => void start()}
            style={[styles.send, task.trim() && !starting ? { backgroundColor: t.accent } : null]}
          >
            <Send size={17} color={task.trim() && !starting ? t.onAccent : t.faint} weight={2.2} />
          </Pressable>
        </KeyboardLift>
      ) : null}
    </Screen>
  );
}

const styles = StyleSheet.create({
  page: { padding: size.gutter, paddingTop: 0, gap: size.gutter, paddingBottom: 32 },
  empty: { padding: 16, gap: 12 },
  group: { padding: 8, gap: 2 },
  groupTitle: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 6,
    paddingTop: 6,
    paddingBottom: 4,
  },
  groupName: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  groupNameText: { fontFamily: font.proseMedium, fontSize: size.detail },
  groupCount: { fontFamily: font.chrome, fontSize: 12, color: t.faint },
  when: { fontSize: size.detail, marginRight: 4 },
  more: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 14,
    paddingVertical: 12,
    borderRadius: size.radiusRow,
  },
  moreText: { fontFamily: font.proseMedium, fontSize: size.body, color: t.accent },
  note: { paddingHorizontal: 6 },
  composer: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 12,
    paddingTop: 8,
    backgroundColor: t.backdrop,
  },
  task: { flex: 1, height: 46, borderRadius: 23, paddingHorizontal: 16 },
  send: { width: 46, height: 46, borderRadius: 23, alignItems: 'center', justifyContent: 'center', backgroundColor: t.sunken },
});
