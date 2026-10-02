'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// One worktree: its figures, and what its agent is waiting on when it is
// waiting.

import { Activity, Answer_Choice, type Changes, FileChange_Status } from '@ket/remote';
import { router, useIsFocused, useLocalSearchParams } from 'expo-router';
import { useEffect, useState } from 'react';
import { Alert, ScrollView, StyleSheet, Text, View } from 'react-native';

import { ConnectionHelp } from '../../../components/ConnectionHelp';
import { RiskBadge } from '../../../components/Risk';
import { AgentMark, TerminalGlyph } from '../../../components/icons';
import { Banner, Bar, Button, Card, Dot, Figure, Heading, Prose, Row, Screen, Value } from '../../../components/ui';
import { agentTerminal, detail, everyWorktree, stateOf } from '../../../lib/activity';
import { useHost } from '../../../lib/hosts';
import { confirmIdentity } from '../../../lib/lock';
import { agentLabel, font, size, t } from '../../../lib/theme';

export default function Worktree() {
  const { host: hostId, worktree: raw } = useLocalSearchParams<{ host: string; worktree: string }>();
  const id = decodeURIComponent(raw);
  const conn = useHost(hostId);
  const [answering, setAnswering] = useState(false);
  const [refusal, setRefusal] = useState('');
  const found = everyWorktree(conn?.snapshot ?? null).find((l) => l.worktree.id === id);

  // What a merge would bring, asked for again whenever the figures move —
  // while this screen is the one showing. A terminal opened from here sits
  // on top of it, and figures move with every edit an agent makes; asked for
  // out of sight, each one spent the desktop's budget for costly requests,
  // and the terminal's own subscribe was refused.
  const focused = useIsFocused();
  const [changes, setChanges] = useState<Changes | null>(null);
  const [merging, setMerging] = useState(false);
  const figures = found
    ? `${found.worktree.filesChanged}:${found.worktree.linesAdded}:${found.worktree.linesRemoved}:${found.worktree.commitsAhead}`
    : '';
  useEffect(() => {
    if (!conn || !figures || !focused) return;
    let stopped = false;
    void conn
      .changes(id)
      .then((reply) => {
        if (!stopped) setChanges(reply);
      })
      .catch(() => {});
    return () => {
      stopped = true;
    };
  }, [conn, id, figures, focused]);

  if (!found) {
    return (
      <Screen>
        <Bar back title={conn?.host.name ?? ''} />
        <View style={styles.page}>
          {conn?.snapshot ? (
            <Card style={styles.pad}>
              <Prose>This worktree is no longer running anything on the desktop.</Prose>
            </Card>
          ) : conn ? (
            <ConnectionHelp conn={conn} />
          ) : null}
        </View>
      </Screen>
    );
  }

  const { project, worktree } = found;
  const agentTerm = agentTerminal(worktree);
  const open = (terminal: bigint) => router.push(`/terminal/${hostId}/${terminal.toString()}`);
  const agentName = worktree.agent ? agentLabel(worktree.agent) : 'The agent';
  const live = worktree.terminals.filter((x) => !x.closed);
  const state = stateOf(worktree.activity);
  const blocked = worktree.activity === Activity.BLOCKED;
  const failed = worktree.activity === Activity.FAILED;
  const question = blocked ? worktree.question : undefined;

  const answer = async (choice: Answer_Choice) => {
    // The prompt's own terminal and id, from the host — never a terminal
    // this screen picks, which could be a different prompt in the same
    // worktree.
    if (!conn || !question) return;
    // "Always" outlives this prompt: with the app lock on, it is you first.
    if (choice === Answer_Choice.ALLOW_ALWAYS && !(await confirmIdentity(`Always allow ${question.tool}`))) return;
    setAnswering(true);
    setRefusal('');
    const why = await conn.answer(question, choice).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setAnswering(false);
    if (why) setRefusal(why);
  };

  return (
    <Screen>
      <Bar back detail={project.name} title={worktree.name} />
      <ScrollView contentContainerStyle={styles.page}>
        <Card style={styles.figures}>
          <Figure label="Agent" style={styles.grow}>
            {state ? <Dot size={7} color={state.color} /> : null}
            <Value color={state?.color ?? t.dim}>{state?.word ?? 'quiet'}</Value>
          </Figure>
          <Figure label="Sessions" style={styles.grow}>
            <Value>{live.length}</Value>
          </Figure>
          {worktree.branch ? (
            <Figure label="Branch" style={styles.branch}>
              <Text style={styles.branchText} numberOfLines={1}>
                {worktree.branch}
              </Text>
            </Figure>
          ) : null}
        </Card>

        <Card style={styles.figures}>
          <Figure label="Changed" style={styles.grow}>
            <Value color={worktree.filesChanged > 0 ? undefined : t.dim}>
              {worktree.filesChanged > 0 ? `${worktree.filesChanged} ${worktree.filesChanged === 1 ? 'file' : 'files'}` : 'clean'}
            </Value>
          </Figure>
          <Figure label="Lines" style={styles.grow}>
            {worktree.linesAdded + worktree.linesRemoved > 0 ? (
              <Text style={styles.lines}>
                <Text style={{ color: t.added }}>+{worktree.linesAdded}</Text>{' '}
                <Text style={{ color: t.removed }}>−{worktree.linesRemoved}</Text>
              </Text>
            ) : (
              <Value color={t.dim}>—</Value>
            )}
          </Figure>
          {worktree.base ? (
            <Figure label={`Ahead of ${worktree.base}`} style={styles.branch}>
              <Value color={worktree.commitsAhead > 0 ? undefined : t.dim}>
                {worktree.commitsAhead === 1 ? '1 commit' : `${worktree.commitsAhead} commits`}
              </Value>
            </Figure>
          ) : null}
        </Card>

        {blocked || failed ? (
          <Banner tone={blocked ? t.attention : t.failed}>
            <View style={styles.askHead}>
              {worktree.agent ? <AgentMark agent={worktree.agent} size={16} /> : null}
              <Text style={styles.askTitle}>
                {question ? asking(agentName, question.tool) : blocked ? `${agentName} is waiting on you` : `${agentName} failed`}
              </Text>
            </View>
            {question ? <RiskBadge risk={question.risk} /> : null}
            {question?.subject ? (
              <View style={styles.well}>
                <Text style={styles.wellText} numberOfLines={12}>
                  {question.tool === 'Bash' ? <Text style={{ color: t.faint }}>$ </Text> : null}
                  {question.subject}
                </Text>
              </View>
            ) : !question && worktree.agentState ? (
              <View style={styles.well}>
                <Text style={styles.wellText}>{detail(worktree)}</Text>
              </View>
            ) : null}
            {refusal ? <Prose color={t.failed}>{refusal}</Prose> : null}
            {question?.answerable && (question.asks.length > 0 || question.plan) ? (
              // A question or a plan has its card in the conversation.
              <>
                <Button
                  kind="primary"
                  title={question.plan ? 'Review the plan' : 'Answer'}
                  onPress={() => router.push(`/session/${hostId}/${question.terminalId.toString()}`)}
                />
                <Button kind="ghost" title="Open the terminal" onPress={() => open(question.terminalId)} />
              </>
            ) : question?.answerable ? (
              <>
                <Button
                  kind="primary"
                  title="Allow once"
                  disabled={answering}
                  onPress={() => void answer(Answer_Choice.ALLOW_ONCE)}
                />
                <View style={styles.choices}>
                  {question.always ? (
                    <Button
                      title="Always allow"
                      disabled={answering}
                      onPress={() => void answer(Answer_Choice.ALLOW_ALWAYS)}
                      style={styles.grow}
                    />
                  ) : null}
                  <Button
                    kind="danger"
                    title="Deny"
                    disabled={answering}
                    onPress={() => void answer(Answer_Choice.DENY)}
                    style={styles.grow}
                  />
                </View>
                <Button kind="ghost" title="Open the terminal" onPress={() => open(question.terminalId)} />
              </>
            ) : question?.terminalId || agentTerm ? (
              <Button
                kind="primary"
                title={blocked ? 'Answer in the terminal' : 'Open the terminal'}
                onPress={() => open(question?.terminalId || agentTerm!.id)}
              />
            ) : null}
          </Banner>
        ) : null}

        {agentTerm && !blocked && !failed ? (
          <Button title="Terminal" icon={<TerminalGlyph size={16} color={t.text} />} onPress={() => open(agentTerm.id)} />
        ) : null}

        {changes && (changes.files.length > 0 || changes.error) ? (
          <Card style={styles.list}>
            <Heading count={changes.files.length}>{changes.base ? `Changes since ${changes.base}` : 'Changes'}</Heading>
            {changes.error ? <Prose style={styles.changesNote}>{changes.error}</Prose> : null}
            {changes.files.map((file) => (
              <Row
                key={file.path}
                small
                title={file.path.split('/').pop() ?? file.path}
                detail={file.path.includes('/') ? file.path.slice(0, file.path.lastIndexOf('/')) : undefined}
                meta={
                  <Text style={styles.fileLines}>
                    {file.binary ? (
                      <Text style={{ color: t.dim }}>binary</Text>
                    ) : (
                      <>
                        <Text style={{ color: t.added }}>+{file.added}</Text>{' '}
                        <Text style={{ color: t.removed }}>−{file.removed}</Text>
                      </>
                    )}
                    {file.status === FileChange_Status.ADDED ? <Text style={{ color: t.dim }}> new</Text> : null}
                    {file.status === FileChange_Status.DELETED ? <Text style={{ color: t.dim }}> gone</Text> : null}
                  </Text>
                }
                chevron
                onPress={() =>
                  router.push(`/diff/${hostId}/${encodeURIComponent(id)}?path=${encodeURIComponent(file.path)}`)
                }
              />
            ))}
            {changes.base ? (
              <View style={styles.actions}>
                <Button
                  kind="primary"
                  title={merging ? 'Merging…' : `Merge into ${changes.base}`}
                  disabled={merging}
                  style={styles.grow}
                  onPress={() =>
                    Alert.alert(
                      `Merge ${worktree.name} into ${changes.base}?`,
                      'Uncommitted changes are committed first, as Merge does on your Mac.',
                      [
                        { text: 'Cancel', style: 'cancel' },
                        {
                          text: 'Merge',
                          onPress: async () => {
                            if (!conn) return;
                            if (!(await confirmIdentity(`Merge ${worktree.name} into ${changes.base}`))) return;
                            setMerging(true);
                            void conn
                              .merge(id)
                              .catch((e: unknown) => (e instanceof Error ? e.message : String(e)))
                              .then((why) => {
                                setMerging(false);
                                if (why) setRefusal(why);
                              });
                          },
                        },
                      ],
                    )
                  }
                />
              </View>
            ) : null}
          </Card>
        ) : null}
      </ScrollView>
    </Screen>
  );
}

/** The prompt's title, in the words for what the tool does. */
function asking(agent: string, tool: string): string {
  switch (tool) {
    case 'Bash':
      return `${agent} wants to run a command`;
    case 'Edit':
    case 'MultiEdit':
    case 'NotebookEdit':
      return `${agent} wants to edit a file`;
    case 'Write':
      return `${agent} wants to write a file`;
    case 'Read':
      return `${agent} wants to read a file`;
    case 'WebFetch':
      return `${agent} wants to open a page`;
    case 'WebSearch':
      return `${agent} wants to search the web`;
    case 'AskUserQuestion':
      return `${agent} has a question`;
    case 'ExitPlanMode':
      return `${agent} has a plan ready`;
    // OpenCode's own permissions, which have no tool of Claude's to be named
    // for: a path outside the worktree, and the same call made again and again.
    case 'external_directory':
      return `${agent} wants to work outside the project`;
    case 'doom_loop':
      return `${agent} wants to repeat the same call`;
    default:
      return `${agent} wants to use ${tool}`;
  }
}

const styles = StyleSheet.create({
  page: { padding: size.gutter, paddingTop: 0, gap: size.gutter, paddingBottom: 32 },
  pad: { padding: 14 },
  grow: { flex: 1 },
  figures: { flexDirection: 'row', gap: 14, padding: 14 },
  branch: { flex: 1.6 },
  branchText: { fontFamily: font.chromeMedium, fontSize: 13.5, color: t.text },
  lines: { fontFamily: font.chromeMedium, fontSize: 13.5 },
  askHead: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  askTitle: { fontFamily: font.proseSemibold, fontSize: size.body, color: t.text },
  well: { backgroundColor: t.sunken, borderRadius: size.radius, paddingHorizontal: 12, paddingVertical: 11 },
  wellText: { fontFamily: font.chrome, fontSize: 13, lineHeight: 20, color: t.text },
  list: { padding: 8, gap: 2 },
  actions: { flexDirection: 'row', gap: 8, padding: 4, paddingTop: 8 },
  choices: { flexDirection: 'row', gap: 8 },
  changesNote: { padding: 8 },
  fileLines: { fontFamily: font.chrome, fontSize: 12, marginLeft: 8 },
});
