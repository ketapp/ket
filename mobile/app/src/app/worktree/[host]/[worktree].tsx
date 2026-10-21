'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// One worktree: its figures, what its agent is waiting on when it is
// waiting, the pull request its branch is open as, the comments on its diff
// waiting to go to the agent, and what a merge would bring — merged from
// here, or pushed and opened as a pull request.

import {
  Activity,
  Answer_Choice,
  type Changes,
  CheckSummary_State,
  FileChange_Status,
  type PullRequestMergeMethod,
  type PullRequestSummary,
} from '@ket/remote';
import { router, useIsFocused, useLocalSearchParams } from 'expo-router';
import { useEffect, useState } from 'react';
import { Alert, Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { ConnectionHelp } from '../../../components/ConnectionHelp';
import { RiskBadge } from '../../../components/Risk';
import { AgentMark, Close, TerminalGlyph } from '../../../components/icons';
import { Banner, Bar, Button, Card, Dot, Figure, Heading, IconButton, Prose, Row, Screen, Value } from '../../../components/ui';
import { agentTerminal, detail, everyWorktree, stateOf } from '../../../lib/activity';
import { checkWord, checksInOrder, prLine } from '../../../lib/checks';
import type { HostConnection } from '../../../lib/connection';
import { useHost } from '../../../lib/hosts';
import { confirmIdentity } from '../../../lib/lock';
import { askMerge, prActions } from '../../../lib/pr-actions';
import { commentsSaid, review, sendReview, useReview } from '../../../lib/review';
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
  // The newest prompt; the list since 1.13, the single field from older desktops.
  const question = blocked ? (worktree.questions[0] ?? worktree.question) : undefined;

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

        {conn && conn.hostMinor >= 15 ? (
          <Button title="New session or shell" onPress={() =>
            router.push(`/new/${hostId}?worktree=${encodeURIComponent(id)}`)
          } />
        ) : null}

        {worktree.pullRequest && conn ? (
          <PullRequest
            pr={worktree.pullRequest}
            conn={conn}
            worktreeId={id}
            agentTerminal={agentTerm?.id ?? null}
          />
        ) : null}

        {conn && conn.hostMinor >= 14 ? <Review conn={conn} hostId={hostId} worktreeId={id} /> : null}

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
                {/* Pushed for review instead: the other way out, as the
                    desktop's merge dialog offers it. Once the branch is open
                    as one, its card above is where it is. */}
                {conn && conn.hostMinor >= 16 && !worktree.pullRequest ? (
                  <Button
                    title="Pull request…"
                    disabled={merging}
                    onPress={() => router.push(`/pull-request/${hostId}/${encodeURIComponent(id)}`)}
                  />
                ) : null}
              </View>
            ) : null}
          </Card>
        ) : null}
      </ScrollView>
    </Screen>
  );
}

/** The pull request a worktree's branch is open as: where it stands, its
 * checks — what failed first, what passed folded away — and, when one
 * failed, the button that hands them to the agent. */
function PullRequest({
  pr,
  conn,
  worktreeId,
  agentTerminal: terminal,
}: {
  pr: PullRequestSummary;
  conn: HostConnection;
  worktreeId: string;
  agentTerminal: bigint | null;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
  const [everyCheck, setEveryCheck] = useState(false);

  const {
    fix: canFix,
    address: canAddress,
    merge: canMerge,
    update: canUpdate,
    ready: canReady,
    reopen: canReopen,
  } = prActions(pr, conn.hostMinor);
  const line = prLine(pr);
  const ordered = checksInOrder(pr);
  const settled = (state: CheckSummary_State) => state !== CheckSummary_State.FAILED && state !== CheckSummary_State.PENDING;
  const folded = everyCheck ? [] : ordered.filter((check) => settled(check.state));
  const shown = everyCheck ? ordered : ordered.filter((check) => !settled(check.state));
  const working = busy !== null;

  const handOver = async (
    kind: 'checks' | 'comments',
    run: () => Promise<{ starting: boolean } | { why: string }>,
  ) => {
    setBusy(kind);
    setSaid(null);
    const handed = await run().catch((e: unknown) => ({ why: e instanceof Error ? e.message : String(e) }));
    setBusy(null);
    if ('why' in handed) {
      setSaid({ text: handed.why, failed: true });
      return;
    }
    const what = kind === 'checks' ? 'failed checks' : 'review comments';
    if (handed.starting) {
      setSaid({
        text: `ket on ${conn.host.name || 'your Mac'} is still starting the agent. It gets the ${what} once it's up.`,
        failed: false,
      });
      return;
    }
    if (terminal !== null) {
      router.push(`/session/${conn.host.id}/${terminal.toString()}`);
      return;
    }
    setSaid({ text: `The agent on ${conn.host.name || 'your Mac'} has the ${what}.`, failed: false });
  };

  const act = async (label: string, run: () => Promise<string | null>) => {
    setBusy(label);
    setSaid(null);
    const why = await run().catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setBusy(null);
    if (why) setSaid({ text: why, failed: true });
  };

  const merge = () =>
    askMerge(pr, (method: PullRequestMergeMethod) => {
      void (async () => {
        if (!(await confirmIdentity(`Merge #${pr.number.toString()}`))) return;
        await act('merge', () => conn.mergePullRequest(worktreeId, method));
      })();
    });

  return (
    <Card style={styles.list}>
      <Heading count={`#${pr.number.toString()}`}>Pull request</Heading>
      {line ? <Prose style={styles.changesNote}>{line}</Prose> : null}
      {shown.map((check, at) => (
        <Row
          key={`${at}:${check.name}`}
          small
          title={check.name}
          state={checkWord(check.state)}
          onPress={check.url ? () => void Linking.openURL(check.url) : undefined}
        />
      ))}
      {folded.length > 0 ? (
        <Pressable
          accessibilityRole="button"
          onPress={() => setEveryCheck(true)}
          style={({ pressed }) => [styles.more, pressed && { backgroundColor: t.selection }]}
        >
          <Text style={styles.moreText}>
            {folded.length === 1 ? 'Show 1 more check' : `Show ${folded.length} more checks`}
          </Text>
        </Pressable>
      ) : null}
      {said ? (
        <Prose color={said.failed ? t.failed : undefined} style={styles.changesNote}>
          {said.text}
        </Prose>
      ) : null}
      <View style={styles.actions}>
        {canFix ? (
          <Button
            kind="primary"
            title={busy === 'checks' ? 'Reading the checks…' : 'Fix checks with agent'}
            disabled={working}
            style={styles.grow}
            onPress={() => void handOver('checks', () => conn.fixChecks(worktreeId))}
          />
        ) : null}
        {canAddress ? (
          <Button
            kind={canFix ? 'ghost' : 'primary'}
            title={busy === 'comments' ? 'Reading the comments…' : 'Address comments with agent'}
            disabled={working}
            style={styles.grow}
            onPress={() => void handOver('comments', () => conn.addressComments(worktreeId))}
          />
        ) : null}
        {canMerge ? (
          <Button
            kind={canFix || canAddress ? 'ghost' : 'primary'}
            title={busy === 'merge' ? 'Merging…' : 'Merge'}
            disabled={working}
            style={styles.grow}
            onPress={merge}
          />
        ) : null}
        {canUpdate ? (
          <Button
            kind="ghost"
            title={busy === 'update' ? 'Updating…' : 'Update branch'}
            disabled={working}
            style={styles.grow}
            onPress={() => void act('update', () => conn.updatePullRequest(worktreeId))}
          />
        ) : null}
        {canReady ? (
          <Button
            kind="ghost"
            title={busy === 'ready' ? 'Marking ready…' : 'Ready for review'}
            disabled={working}
            style={styles.grow}
            onPress={() => void act('ready', () => conn.readyPullRequest(worktreeId))}
          />
        ) : null}
        {canReopen ? (
          <Button
            kind="ghost"
            title={busy === 'reopen' ? 'Reopening…' : 'Reopen'}
            disabled={working}
            style={styles.grow}
            onPress={() => void act('reopen', () => conn.reopenPullRequest(worktreeId))}
          />
        ) : null}
        <Button
          kind={canFix || canAddress || canMerge ? 'ghost' : undefined}
          title="Open on GitHub"
          style={styles.grow}
          onPress={() => void Linking.openURL(pr.url)}
        />
      </View>
    </Card>
  );
}

/** The comments written on the worktree's diff, waiting to go to its agent
 * as one review (desktop 1.14): each opens its file's diff, and Send to agent
 * sends them all — see lib/review.ts. Nothing while there are none. */
function Review({ conn, hostId, worktreeId }: { conn: HostConnection; hostId: string; worktreeId: string }) {
  const comments = useReview(hostId, worktreeId);
  const [sending, setSending] = useState(false);
  // How the last send went, said only until the review changes again.
  const [sent, setSent] = useState<{ text: string; failed: boolean; after: typeof comments } | null>(null);
  const said = sent && sent.after === comments ? sent : null;
  if (comments.length === 0 && !said) return null;

  const send = async () => {
    setSending(true);
    setSent(null);
    const result = await sendReview(conn, hostId, worktreeId);
    setSent({ ...result, after: review.list(hostId, worktreeId) });
    setSending(false);
  };
  const clear = () =>
    Alert.alert(`Clear ${commentsSaid(comments.length)}?`, 'They are not sent to the agent.', [
      { text: 'Cancel', style: 'cancel' },
      {
        text: 'Clear',
        style: 'destructive',
        onPress: () => review.remove(hostId, worktreeId),
      },
    ]);

  return (
    <Card style={styles.list}>
      <Heading count={comments.length > 0 ? comments.length : undefined}>Review</Heading>
      {comments.map((comment) => (
        <Row
          key={comment.id}
          small
          title={comment.path.split('/').pop() ?? comment.path}
          detail={[comment.lines, comment.comment.split('\n')[0]].filter(Boolean).join(' · ')}
          meta={
            <IconButton label="Remove from the review" onPress={() => review.remove(hostId, worktreeId, [comment.id])}>
              <Close size={16} color={t.faint} />
            </IconButton>
          }
          onPress={() =>
            router.push(`/diff/${hostId}/${encodeURIComponent(worktreeId)}?path=${encodeURIComponent(comment.path)}`)
          }
        />
      ))}
      {said ? (
        <Prose color={said.failed ? t.failed : undefined} style={styles.changesNote}>
          {said.text}
        </Prose>
      ) : null}
      {comments.length > 0 ? (
        <View style={styles.actions}>
          <Button
            kind="primary"
            title={sending ? 'Sending…' : 'Send to agent'}
            disabled={sending}
            style={styles.grow}
            onPress={() => void send()}
          />
          <Button kind="ghost" title="Clear" disabled={sending} onPress={clear} />
        </View>
      ) : null}
    </Card>
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
  // In from the edge a row's name starts at.
  more: { minHeight: size.rowSm, justifyContent: 'center', paddingHorizontal: 14, borderRadius: size.radiusRow },
  moreText: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  fileLines: { fontFamily: font.chrome, fontSize: 12, marginLeft: 8 },
});
