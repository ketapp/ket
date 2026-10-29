'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// One worktree: what it has changed since its base, and how to land it —
// merged from here, or pushed and opened as a pull request — docked at the
// foot. Once a pull request is open, where it stands sits over the changes
// and the foot hands its checks or comments to the agent. What the agent is
// waiting on, when it is waiting, and the comments on its diff waiting to go
// to the agent, each have their card above. Everything else the worktree
// offers — its terminal, a new session, pinning, deleting, the rest of the
// pull request's actions — waits in the bar's menu, as the desktop's
// worktree menu has it.

import {
  Activity,
  Answer_Choice,
  type Changes,
  CheckSummary_State,
  FileChange_Status,
  type PullRequestMergeMethod,
  type PullRequestSummary,
  PullRequestSummary_State,
} from '@ket/remote';
import { router, useIsFocused, useLocalSearchParams } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import { Alert, Linking, Pressable, ScrollView, StyleSheet, Text, View, type ViewInstance } from 'react-native';

import { ConnectionHelp } from '../../../components/ConnectionHelp';
import { FloatMenu, type MenuItem, type MenuSection, type Place, placeOf } from '../../../components/FloatMenu';
import { RiskBadge } from '../../../components/Risk';
import { worktreeMenu } from '../../../components/WorktreeList';
import {
  AgentMark,
  CheckCircle,
  Close,
  Comment,
  External,
  Merge,
  More,
  Plus,
  PullRequestGlyph,
  TerminalWindow,
} from '../../../components/icons';
import { Banner, Bar, Button, Card, Heading, IconButton, IconCluster, Prose, Row, Screen } from '../../../components/ui';
import { ASLEEP, WAKING, agentTerminal, asleepName, detail, everyWorktree } from '../../../lib/activity';
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
  // The permission just answered, by its id: its buttons go on the tap and
  // stay gone until the desktop's next snapshot takes the question away.
  // Back only if the desktop would not take the answer, with its reason.
  const [answered, setAnswered] = useState<bigint | null>(null);
  const [refusal, setRefusal] = useState('');
  // The asleep tab the desktop is waking for this phone, by its key.
  const [waking, setWaking] = useState('');
  // The bar's menu, hung under its button while it is open.
  const [menu, setMenu] = useState<Place | null>(null);
  const more = useRef<ViewInstance>(null);
  const found = everyWorktree(conn?.snapshot ?? null).find((l) => l.worktree.id === id);

  // What a merge would bring, asked for again whenever the figures move —
  // while this screen is the one showing. A terminal opened from here sits
  // on top of it, and figures move with every edit an agent makes; asked for
  // out of sight, each one spent the desktop's budget for costly requests,
  // and the terminal's own subscribe was refused.
  const focused = useIsFocused();
  const [changes, setChanges] = useState<Changes | null>(null);
  const [merging, setMerging] = useState(false);
  // Why the desktop would not merge from here, under the changes.
  const [mergeWhy, setMergeWhy] = useState('');
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

  const pr = found?.worktree.pullRequest;
  const agentTerm = found ? agentTerminal(found.worktree) : undefined;
  const actions = usePullRequest(pr, conn, id, agentTerm?.id ?? null);

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
  const open = (terminal: bigint) => router.push(`/terminal/${hostId}/${terminal.toString()}`);
  // Wakes an asleep tab on the desktop and opens its chat once its terminal
  // is there. The desktop says why when it won't.
  const wake = async (key: string) => {
    if (!conn || waking) return;
    setWaking(key);
    const woke = await conn
      .wakeTerminal(id, key)
      .catch((e: unknown) => ({ why: e instanceof Error ? e.message : String(e) }));
    setWaking('');
    if ('why' in woke) {
      Alert.alert('Couldn’t wake the agent', woke.why);
      return;
    }
    await conn.awaitTerminal(woke.terminalId);
    router.push(`/session/${hostId}/${woke.terminalId.toString()}`);
  };
  const canWake = conn !== null && conn.hostMinor >= 23;
  const agentName = worktree.agent ? agentLabel(worktree.agent) : 'The agent';
  const live = worktree.terminals.filter((x) => !x.closed);
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
    // Gone on the tap: a second tap on buttons still up was a second key.
    setAnswered(question.id);
    setRefusal('');
    const why = await conn.answer(question, choice).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    if (why) {
      setAnswered(null);
      setRefusal(why);
    }
  };

  const mergeHere = () =>
    Alert.alert(
      `Merge ${worktree.name} into ${changes?.base}?`,
      'Uncommitted changes are committed first, as Merge does on your Mac.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Merge',
          onPress: async () => {
            if (!conn) return;
            if (!(await confirmIdentity(`Merge ${worktree.name} into ${changes?.base}`))) return;
            setMerging(true);
            setMergeWhy('');
            void conn
              .merge(id)
              .catch((e: unknown) => (e instanceof Error ? e.message : String(e)))
              .then((why) => {
                setMerging(false);
                if (why) setMergeWhy(why);
              });
          },
        },
      ],
    );

  // The foot: with a pull request open, its first thing to do and GitHub;
  // without one — or with one closed, whose branch can still land from
  // here — merging from here and opening one. Nothing while there is nothing
  // to land. The keys are what the menu leaves out, being here.
  const base = changes?.base ?? '';
  const files = changes?.files ?? [];
  const landable = base !== '' && (files.length > 0 || worktree.commitsAhead > 0);
  const inFoot: string[] = [];
  let foot: React.ReactNode = null;
  if (pr && conn && pr.state === PullRequestSummary_State.OPEN) {
    const { can, busy, handOver, merge } = actions;
    const working = busy !== null;
    let lead: { key: string; title: string; icon: React.ReactNode; onPress: () => void } | null = null;
    if (can.fix) {
      lead = {
        key: 'fix',
        title: busy === 'checks' ? 'Reading the checks…' : 'Fix checks with agent',
        icon: <CheckCircle size={16} color={t.onAccent} />,
        onPress: () => void handOver('checks', () => conn.fixChecks(id)),
      };
    } else if (can.address) {
      lead = {
        key: 'address',
        // Shorter than the menu says it, to share the foot with GitHub.
        title: busy === 'comments' ? 'Reading the comments…' : 'Hand comments to agent',
        icon: <Comment size={16} color={t.onAccent} />,
        onPress: () => void handOver('comments', () => conn.addressComments(id)),
      };
    } else if (can.merge) {
      lead = {
        key: 'merge',
        title: busy === 'merge' ? 'Merging…' : 'Merge pull request',
        icon: <Merge size={16} color={t.onAccent} />,
        onPress: merge,
      };
    }
    if (lead) inFoot.push(lead.key);
    foot = (
      <View style={styles.foot}>
        {lead ? (
          <Button kind="primary" title={lead.title} icon={lead.icon} disabled={working} style={styles.grow} onPress={lead.onPress} />
        ) : null}
        <Button
          title={lead ? 'GitHub' : 'Open on GitHub'}
          icon={<External size={16} color={t.text} />}
          style={lead ? undefined : styles.grow}
          onPress={() => void Linking.openURL(pr.url)}
        />
      </View>
    );
  } else if (landable) {
    // Pushed for review instead: the other way out, as the desktop's merge
    // dialog offers it. Not before 1.16, which opens them, and not for a
    // branch already open as one and closed: the menu offers reopening.
    const opens = conn !== null && conn.hostMinor >= 16 && !pr;
    if (opens) inFoot.push('open-pr');
    foot = (
      <View style={styles.foot}>
        <Button
          kind="primary"
          title={merging ? 'Merging…' : `Merge into ${base}`}
          icon={<Merge size={16} color={t.onAccent} />}
          disabled={merging}
          style={styles.grow}
          onPress={mergeHere}
        />
        {opens ? (
          <Button
            title="Pull request…"
            icon={<PullRequestGlyph size={16} color={t.text} />}
            disabled={merging}
            onPress={() => router.push(`/pull-request/${hostId}/${encodeURIComponent(id)}`)}
          />
        ) : null}
      </View>
    );
  }

  // The bar's menu: into the worktree's terminal or a new one first, then
  // what holding its row on Worktrees offers, less what the foot already has.
  const sections: MenuSection[] = [];
  if (conn) {
    const first: MenuItem[] = [];
    const terminal = agentTerm ?? live[0];
    if (terminal) {
      first.push({
        key: 'terminal',
        label: 'Open the terminal',
        icon: <TerminalWindow size={17} color={t.dim} />,
        onPress: () => open(terminal.id),
      });
    }
    if (conn.hostMinor >= 15) {
      first.push({
        key: 'new',
        label: 'New session or shell',
        icon: <Plus size={17} color={t.dim} />,
        onPress: () => router.push(`/new/${hostId}?worktree=${encodeURIComponent(id)}`),
      });
    }
    if (first.length > 0) sections.push({ items: first });
    // Deleted from here: this screen has nothing left to show, so it goes
    // back at once; a refusal is said in its alert wherever that lands.
    const going = (_id: string, away: boolean) => {
      if (away && router.canGoBack()) router.back();
    };
    for (const section of worktreeMenu(conn, worktree, (href) => router.push(href), going)) {
      const items = section.items.filter((item) => !inFoot.includes(item.key));
      if (items.length > 0) sections.push({ ...section, items });
    }
  }

  // The lines are the listed files' own: the worktree's figures count only
  // what is uncommitted, and the list is everything since the base.
  const added = files.reduce((n, file) => n + file.added, 0);
  const removed = files.reduce((n, file) => n + file.removed, 0);
  const ahead = worktree.commitsAhead === 1 ? '1 commit' : `${worktree.commitsAhead} commits`;
  const clean = changes !== null && files.length === 0 && worktree.commitsAhead === 0;

  return (
    <Screen>
      <Bar
        back
        detail={project.name}
        title={worktree.name}
        right={
          sections.length > 0 ? (
            <View ref={more} collapsable={false}>
              <IconCluster
                actions={[{ label: 'More', icon: <More size={18} color={t.text} />, onPress: () => placeOf(more.current, setMenu) }]}
              />
            </View>
          ) : null
        }
      />
      <ScrollView contentContainerStyle={styles.page}>
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
                {question.id !== answered ? (
                  <>
                    <Button kind="primary" title="Allow once" onPress={() => void answer(Answer_Choice.ALLOW_ONCE)} />
                    <View style={styles.choices}>
                      {question.always ? (
                        <Button title="Always allow" onPress={() => void answer(Answer_Choice.ALLOW_ALWAYS)} style={styles.grow} />
                      ) : null}
                      <Button kind="danger" title="Deny" onPress={() => void answer(Answer_Choice.DENY)} style={styles.grow} />
                    </View>
                  </>
                ) : null}
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

        {worktree.asleep.length > 0 ? (
          <Card>
            <Heading count={worktree.asleep.length}>Asleep</Heading>
            {worktree.asleep.map((tab) => (
              <Row
                key={tab.key}
                mark={tab.agent ? <AgentMark agent={tab.agent} size={16} /> : null}
                title={asleepName(tab)}
                detail={
                  waking === tab.key
                    ? 'Waking on your Mac…'
                    : tab.sinceMs > 0n
                      ? `Asleep since ${new Date(Number(tab.sinceMs)).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })} · ${Math.round(Number(tab.freedBytes) / 1_048_576)} MB freed`
                      : 'Stopped with ket’s host · tap to start it again'
                }
                state={waking === tab.key ? WAKING : ASLEEP}
                chevron={canWake}
                onPress={canWake ? () => void wake(tab.key) : undefined}
              />
            ))}
          </Card>
        ) : null}

        {pr ? <PullRequest pr={pr} said={actions.said} /> : null}

        {conn && conn.hostMinor >= 14 ? <Review conn={conn} hostId={hostId} worktreeId={id} /> : null}

        <Card style={styles.list}>
          <Heading count={changes ? files.length : undefined}>
            {worktree.base ? `Changes since ${worktree.base}` : 'Changes'}
          </Heading>
          {changes && !clean && worktree.base ? (
            <Text style={styles.figures}>
              <Text style={{ color: t.added }}>+{added}</Text>{' '}
              <Text style={{ color: t.removed }}>−{removed}</Text>
              {` · ${ahead} ahead of ${worktree.base}`}
            </Text>
          ) : null}
          {changes?.error ? <Prose style={styles.changesNote}>{changes.error}</Prose> : null}
          {mergeWhy ? (
            <Prose color={t.failed} style={styles.changesNote}>
              {mergeWhy}
            </Prose>
          ) : null}
          {clean && files.length === 0 ? (
            <Prose style={styles.changesNote}>{worktree.base ? `Nothing changed since ${worktree.base}.` : 'Nothing changed.'}</Prose>
          ) : null}
          {files.map((file) => (
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
        </Card>
      </ScrollView>
      {foot}
      <FloatMenu
        open={menu !== null}
        onClose={() => setMenu(null)}
        anchor={menu}
        label={`${worktree.name} actions`}
        sections={sections}
      />
    </Screen>
  );
}

/** A pull request's actions, as the foot offers them: what is being done,
 * what the last one said, and the doing. Idle with no pull request. */
function usePullRequest(pr: PullRequestSummary | undefined, conn: HostConnection | null, worktreeId: string, terminal: bigint | null) {
  const [busy, setBusy] = useState<string | null>(null);
  const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
  const can = pr && conn ? prActions(pr, conn.hostMinor) : { fix: false, address: false, merge: false, update: false, ready: false, reopen: false };

  // Handed to the agent: off to its conversation to watch it work, or word
  // of where the handing stands when there is none to open.
  const handOver = async (kind: 'checks' | 'comments', run: () => Promise<{ starting: boolean } | { why: string }>) => {
    if (!conn) return;
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

  const merge = () => {
    if (!pr || !conn) return;
    askMerge(pr, (method: PullRequestMergeMethod) => {
      void (async () => {
        if (!(await confirmIdentity(`Merge #${pr.number.toString()}`))) return;
        await act('merge', () => conn.mergePullRequest(worktreeId, method));
      })();
    });
  };

  return { can, busy, said, handOver, act, merge };
}

/** The pull request a worktree's branch is open as: where it stands, its
 * checks — what failed first, what passed folded away — and what its last
 * action from the foot said. */
function PullRequest({ pr, said }: { pr: PullRequestSummary; said: { text: string; failed: boolean } | null }) {
  const [everyCheck, setEveryCheck] = useState(false);
  const line = prLine(pr);
  const ordered = checksInOrder(pr);
  const settled = (state: CheckSummary_State) => state !== CheckSummary_State.FAILED && state !== CheckSummary_State.PENDING;
  const folded = everyCheck ? [] : ordered.filter((check) => settled(check.state));
  const shown = everyCheck ? ordered : ordered.filter((check) => !settled(check.state));

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
    case 'request_user_input':
    case 'Dialog':
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
  askHead: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  askTitle: { fontFamily: font.proseSemibold, fontSize: size.body, color: t.text },
  well: { backgroundColor: t.sunken, borderRadius: size.radius, paddingHorizontal: 12, paddingVertical: 11 },
  wellText: { fontFamily: font.chrome, fontSize: 13, lineHeight: 20, color: t.text },
  list: { padding: 8, gap: 2 },
  // Under the heading, on its left edge: the lines, and the commits ahead.
  figures: { fontFamily: font.chrome, fontSize: size.detail, color: t.dim, paddingHorizontal: 6, paddingBottom: 6 },
  actions: { flexDirection: 'row', gap: 8, padding: 4, paddingTop: 8 },
  // Docked under the list, clear of the home indicator.
  foot: { flexDirection: 'row', gap: 8, paddingHorizontal: size.gutter, paddingTop: size.gutter, paddingBottom: 34 },
  choices: { flexDirection: 'row', gap: 8 },
  changesNote: { padding: 8 },
  // In from the edge a row's name starts at.
  more: { minHeight: size.rowSm, justifyContent: 'center', paddingHorizontal: 14, borderRadius: size.radiusRow },
  moreText: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  fileLines: { fontFamily: font.chrome, fontSize: 12, marginLeft: 8 },
});
