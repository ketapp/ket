'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// Needs you: every worktree, on every paired desktop, whose agent is waiting
// on a person — and, under them, the ones that failed. The one list worth
// opening the app for. Each prompt waiting is a card of its own, answered
// right there; its head opens the chat for the conversation around it.

import { Activity, Answer_Choice, type Question } from '@ket/remote';
import { router } from 'expo-router';
import { type ReactNode, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { HORIZON, Horizon } from '../../components/Horizon';
import { AgentMark, Check, Chevron, Close, TerminalWindow } from '../../components/icons';
import { RiskBadge } from '../../components/Risk';
import { Button, Card, Dot, Heading, MARK_GAP, MARK_SLOT, Prose, Row, Screen, TAB_BAR_ROOM, TabBar } from '../../components/ui';
import { type Located, agentOf, detail, detailColor, everyWorktree, promptKind, stateOf } from '../../lib/activity';
import type { HostConnection, PromptAnswer } from '../../lib/connection';
import { useHosts } from '../../lib/hosts';
import { confirmIdentity } from '../../lib/lock';
import { agentLabel, font, size, t } from '../../lib/theme';

export default function Needs() {
  const conns = useHosts() ?? [];
  const many = conns.length > 1;
  const of = (activity: Activity) =>
    conns.flatMap((conn) =>
      everyWorktree(conn.snapshot)
        .filter(({ worktree }) => worktree.activity === activity)
        .map((located) => ({ conn, ...located })),
    );
  const waiting = of(Activity.BLOCKED);
  const failed = of(Activity.FAILED);
  // One card per prompt: several agents in a worktree can each be waiting.
  // A desktop from before 1.13 sends only the newest.
  const prompts = waiting.flatMap((located) => {
    const { worktree } = located;
    const asked = worktree.questions.length > 0 ? worktree.questions : [worktree.question];
    return asked.map((question, at) => ({ ...located, question, at }));
  });

  // Empty, the tab is the desktop's welcome picture: a planet rising from
  // under the tab bar, the words in its sky.
  const quiet = waiting.length === 0 && failed.length === 0;
  return (
    <Screen>
      {quiet ? (
        <View style={styles.sky}>
          <Horizon />
          <View style={styles.words}>
            <Prose color={t.text} style={styles.title}>
              Nothing needs you.
            </Prose>
            <Prose style={styles.note}>When an agent asks for permission or stops on a question, it shows up here.</Prose>
          </View>
        </View>
      ) : (
        <ScrollView contentContainerStyle={styles.page}>
          {prompts.length > 0 ? (
            <View style={styles.bare}>
              <Heading color={t.attention} count={prompts.length}>
                Waiting on you
              </Heading>
            </View>
          ) : null}
          {prompts.map(({ conn, project, worktree, question, at }) => (
            <Waiting
              key={`${conn.host.id}/${worktree.id}/${question?.id.toString() ?? at}`}
              conn={conn}
              many={many}
              project={project}
              worktree={worktree}
              question={question}
            />
          ))}
          {failed.length > 0 ? (
            <Card style={styles.card}>
              <Heading color={t.failed} count={failed.length}>
                Failed
              </Heading>
              {failed.map(({ conn, project, worktree }) => (
                <Row
                  key={`${conn.host.id}/${worktree.id}`}
                  mark={agentOf(worktree) ? <AgentMark agent={agentOf(worktree)} size={16} /> : null}
                  title={worktree.name}
                  detail={[many ? conn.host.name : null, project.name, detail(worktree)].filter(Boolean).join(' · ')}
                  detailColor={detailColor(worktree.activity)}
                  state={stateOf(worktree.activity)}
                  onPress={() => router.push(`/worktree/${conn.host.id}/${encodeURIComponent(worktree.id)}`)}
                />
              ))}
            </Card>
          ) : null}
        </ScrollView>
      )}
      <TabBar active="needs" needs={waiting.length} />
    </Screen>
  );
}

/** One prompt waiting on you, as a card: whose it is at the head, which
 * opens the chat, then what it asks with its answers. The desktop takes an
 * answer only while that terminal still shows that very prompt, and says
 * why when not. What can't be answered here sends you to the chat or the
 * terminal: a plan to change, several questions, a pick of many. */
function Waiting({
  conn,
  many,
  project,
  worktree,
  question,
}: Located & { conn: HostConnection; many: boolean; question: Question | undefined }) {
  const [chosen, setChosen] = useState<Answer_Choice>();
  const [picked, setPicked] = useState<number>();
  const [approved, setApproved] = useState(false);
  const [why, setWhy] = useState('');
  const host = conn.host.id;
  const agent = agentOf(worktree);
  const terminal = question?.terminalId ? question.terminalId.toString() : '';
  const openChat = () =>
    router.push(terminal ? `/session/${host}/${terminal}` : `/worktree/${host}/${encodeURIComponent(worktree.id)}`);
  const openTerminal = () => router.push(`/terminal/${host}/${terminal}`);
  const failed = (e: unknown) => (e instanceof Error ? e.message : String(e));

  const answer = async (choice: Answer_Choice) => {
    if (!question) return;
    // "Always" outlives this prompt: with the app lock on, it is you first.
    if (choice === Answer_Choice.ALLOW_ALWAYS && !(await confirmIdentity(`Always allow ${question.tool}`))) return;
    // Gone on the tap: a second tap on a card still up was a second key.
    setChosen(choice);
    setWhy('');
    const refused = await conn.answer(question, choice).catch(failed);
    if (refused) {
      setChosen(undefined);
      setWhy(refused);
    }
  };
  // A question's pick or a plan's approval; `undo` takes the tap back when
  // the desktop refuses it.
  const respond = async (reply: PromptAnswer, undo: () => void) => {
    if (!question) return;
    setWhy('');
    const refused = await conn.respond(question, reply).catch(failed);
    if (refused) {
      undo();
      setWhy(refused);
    }
  };

  const kind = question ? promptKind(question.tool) : undefined;
  const body = ((): ReactNode => {
    if (!question || !terminal) return <Asks>{detail(worktree)}</Asks>;
    const unanswerable = !question.answerable || (kind === 'ask' && question.asks.length === 0);
    if (kind === 'plan') {
      return (
        <>
          <Asks>Has a plan ready</Asks>
          {question.subject ? <Text style={styles.lead}>{question.subject}</Text> : null}
          <View style={styles.buttons}>
            {unanswerable ? null : (
              <Button
                kind="primary"
                title="Approve"
                mark={approved ? <Check size={20} color={t.onAccent} weight={2.4} /> : undefined}
                onPress={() => {
                  setApproved(true);
                  void respond({ approve: true }, () => setApproved(false));
                }}
                style={styles.grow}
              />
            )}
            <Button title="Read the plan" onPress={openChat} style={styles.grow} />
          </View>
        </>
      );
    }
    if (kind === 'ask') {
      const first = question.asks[0];
      // One question, one pick: a tap is the answer. Anything more takes the
      // chat's own card.
      const oneTap = !unanswerable && question.asks.length === 1 && !first.multiSelect;
      return (
        <>
          <Asks>{first?.header ? `Asks · ${first.header}` : 'Asks'}</Asks>
          {question.asks.length > 0 ? (
            question.asks.map((asked, i) => (
              <Text key={i} style={styles.question}>
                {asked.question}
              </Text>
            ))
          ) : question.subject ? (
            <Text style={styles.question}>{question.subject}</Text>
          ) : null}
          {oneTap ? (
            <View style={styles.options}>
              {first.choices.map((choice, j) => (
                <Option
                  key={j}
                  label={choice.label}
                  description={choice.description}
                  picked={picked === j}
                  disabled={picked !== undefined}
                  onPress={() => {
                    setPicked(j);
                    void respond({ picks: [{ choices: [j], other: '' }] }, () => setPicked(undefined));
                  }}
                />
              ))}
            </View>
          ) : unanswerable ? (
            <Button title="Answer in the terminal" icon={<TerminalWindow size={16} color={t.text} />} onPress={openTerminal} />
          ) : (
            <Button kind="primary" title="Answer in the chat" onPress={openChat} />
          )}
          {oneTap && question.tool === 'AskUserQuestion' ? (
            // A dialog read off the agent's screen, or Codex's form, takes one
            // of its own options and nothing else.
            <Pressable onPress={openChat} accessibilityRole="link" hitSlop={8}>
              <Text style={styles.more}>Write your own in the chat</Text>
            </Pressable>
          ) : null}
        </>
      );
    }
    const done = chosen !== undefined;
    const markOf = (choice: Answer_Choice, color: string) =>
      chosen !== choice ? undefined : choice === Answer_Choice.DENY ? (
        <Close size={20} color={color} weight={2.4} />
      ) : (
        <Check size={20} color={color} weight={2.4} />
      );
    return (
      <>
        <Asks>{question.tool === 'Bash' ? 'Wants to run' : `Wants to use ${question.tool}`}</Asks>
        <RiskBadge risk={question.risk} />
        {question.subject ? (
          <View style={styles.well}>
            <Text style={styles.code} numberOfLines={8}>
              {question.subject}
            </Text>
          </View>
        ) : null}
        {unanswerable ? (
          <Button title="Answer in the terminal" icon={<TerminalWindow size={16} color={t.text} />} onPress={openTerminal} />
        ) : (
          <View style={styles.buttons}>
            <Button
              kind="primary"
              title="Allow once"
              mark={markOf(Answer_Choice.ALLOW_ONCE, t.onAccent)}
              disabled={done}
              onPress={() => void answer(Answer_Choice.ALLOW_ONCE)}
              style={styles.grow}
            />
            {question.always ? (
              <Button
                title="Always"
                mark={markOf(Answer_Choice.ALLOW_ALWAYS, t.text)}
                disabled={done}
                onPress={() => void answer(Answer_Choice.ALLOW_ALWAYS)}
                style={styles.grow}
              />
            ) : null}
            <Button
              kind="danger"
              title="Deny"
              mark={markOf(Answer_Choice.DENY, t.failed)}
              disabled={done}
              onPress={() => void answer(Answer_Choice.DENY)}
              style={styles.grow}
            />
          </View>
        )}
      </>
    );
  })();

  return (
    <View style={styles.prompt}>
      <Pressable
        onPress={openChat}
        accessibilityRole="button"
        accessibilityLabel={`${worktree.name}, open the chat`}
        style={({ pressed }) => [styles.head, pressed && { backgroundColor: t.selection }]}
      >
        <View style={styles.mark}>{agent ? <AgentMark agent={agent} size={16} /> : null}</View>
        <View style={styles.headText}>
          <Text style={styles.name} numberOfLines={1}>
            {worktree.name}
          </Text>
          <Text style={styles.detail} numberOfLines={1}>
            {[many ? conn.host.name : null, project.name, agent ? agentLabel(agent) : null].filter(Boolean).join(' · ')}
          </Text>
        </View>
        <Chevron size={14} color={t.faint} weight={2} />
      </Pressable>
      {body}
      {why ? <Text style={styles.why}>{why}</Text> : null}
    </View>
  );
}

/** What the prompt is, in the waiting colour: "Wants to run", "Asks". */
function Asks({ children }: { children: ReactNode }) {
  return (
    <View style={styles.asks}>
      <Dot color={t.attention} size={6} />
      <Text style={styles.asksText} numberOfLines={2}>
        {children}
      </Text>
    </View>
  );
}

/** One of a question's options; tapped, it is the answer, and wears a tick
 * until the desktop's next snapshot takes the card away. */
function Option({
  label,
  description,
  picked,
  disabled,
  onPress,
}: {
  label: string;
  description: string;
  picked: boolean;
  disabled: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled, selected: picked }}
      style={({ pressed }) => [
        styles.option,
        (pressed || picked) && { backgroundColor: t.hover },
        disabled && !picked && { opacity: 0.55 },
      ]}
    >
      <View style={styles.optionText}>
        <Text style={styles.optionLabel}>{label}</Text>
        {description ? <Text style={styles.detail}>{description}</Text> : null}
      </View>
      {picked ? <Check size={18} color={t.text} weight={2.4} /> : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  // No heading above it: the tab bar already says where this is. Starts where
  // the Worktrees tab's page does.
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: TAB_BAR_ROOM },
  card: { padding: 8, gap: 2 },
  // The heading over the prompt cards, at the inset it has inside a card.
  bare: { paddingHorizontal: 6, marginBottom: -4 },
  prompt: {
    backgroundColor: t.card,
    borderWidth: 1,
    borderColor: t.rule,
    borderRadius: size.radiusCard,
    paddingTop: 6,
    paddingHorizontal: size.pad,
    paddingBottom: size.pad,
    gap: 10,
  },
  head: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: MARK_GAP,
    minHeight: 52,
    marginHorizontal: -4,
    paddingHorizontal: 4,
    borderRadius: size.radiusRow,
  },
  mark: { width: MARK_SLOT, alignItems: 'center' },
  headText: { flex: 1, minWidth: 0, gap: 3 },
  name: { fontFamily: font.chrome, fontSize: size.name, color: t.text },
  detail: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  asks: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  asksText: { flex: 1, fontFamily: font.proseMedium, fontSize: size.label, color: t.attention },
  well: { backgroundColor: t.sunken, borderRadius: size.radius, paddingVertical: 10, paddingHorizontal: 12 },
  code: { fontFamily: font.code, fontSize: 12.5, lineHeight: 18, color: t.text },
  lead: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  question: { fontFamily: font.proseSemibold, fontSize: size.title, lineHeight: 21, color: t.text },
  buttons: { flexDirection: 'row', gap: 8 },
  grow: { flex: 1 },
  options: { gap: 8 },
  option: {
    minHeight: size.control,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: size.radius,
    borderWidth: 1,
    borderColor: t.border,
  },
  optionText: { flex: 1, minWidth: 0, gap: 2 },
  optionLabel: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  more: { fontFamily: font.prose, fontSize: size.label, color: t.dim, textAlign: 'center' },
  why: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.failed },
  sky: { flex: 1 },
  // The words sit centred in the sky above the planet's top.
  words: {
    position: 'absolute',
    top: 0,
    left: 0,
    right: 0,
    height: `${HORIZON * 100}%`,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
    gap: 6,
  },
  title: { fontSize: size.title, textAlign: 'center' },
  note: { textAlign: 'center' },
});
