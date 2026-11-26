import { Answer_Choice, type Question } from '@ket/remote';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, TouchableOpacity, View } from 'react-native';

import { Check, Close, TerminalWindow } from './icons';
import { RiskBadge } from './Risk';
import { AgentTile, Button, Row, RuledCard } from './ui';
import { type Located, agentOf, colorOf, detail, promptKind } from '../lib/activity';
import type { HostConnection, PromptAnswer } from '../lib/connection';
import { useHostState } from '../lib/hosts';
import { confirmIdentity } from '../lib/lock';
import { agentLabel, font, sessionColor, size, t } from '../lib/theme';
import { PRESSED, surface } from './ui/surface';

/** One prompt waiting on you, as a ruled card: whose it is in the row at
 * its head, which opens the chat, then what it asks with its answers. The desktop takes an
 * answer only while that terminal still shows that very prompt, and says
 * why when not. What can't be answered here sends you to the chat or the
 * terminal: a plan to change, several questions, a pick of many. */
export function WaitingPrompt({
  conn,
  many,
  project,
  worktree,
  question,
  onOpen,
}: Located & {
  conn: HostConnection;
  many?: boolean;
  question: Question | undefined;
  onOpen: (href: string) => void;
}) {
  const [chosen, setChosen] = useState<Answer_Choice>();
  const [picked, setPicked] = useState<number>();
  const [approved, setApproved] = useState(false);
  const [why, setWhy] = useState('');
  const hostState = useHostState(conn);
  const host = conn.id;
  const agent = agentOf(worktree);
  const terminal = question?.terminalId ? question.terminalId.toString() : '';
  const openChat = () =>
    onOpen(terminal ? `/session/${host}/${terminal}` : `/worktree/${host}/${encodeURIComponent(worktree.id)}`);
  const openTerminal = () => onOpen(`/terminal/${host}/${terminal}`);
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
                capsule
                title="Approve"
                disabled={approved}
                mark={approved ? <Check size={20} color={t.onAccent} weight={2.4} /> : undefined}
                onPress={() => {
                  setApproved(true);
                  void respond({ approve: true }, () => setApproved(false));
                }}
                style={styles.grow}
              />
            )}
            <Button capsule title="Read the plan" onPress={openChat} style={styles.grow} />
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
            <Button capsule title="Answer in the terminal" icon={<TerminalWindow size={16} color={t.text} />} onPress={openTerminal} />
          ) : (
            <Button kind="primary" capsule title="Answer in the chat" onPress={openChat} />
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
        <View style={styles.asksLine}>
          <Asks>{question.tool === 'Bash' ? 'Wants to run' : `Wants to use ${question.tool}`}</Asks>
          <RiskBadge risk={question.risk} />
        </View>
        {question.subject ? (
          <View style={styles.well}>
            <Text style={styles.code} numberOfLines={8}>
              {question.subject}
            </Text>
          </View>
        ) : null}
        {unanswerable ? (
          <Button capsule title="Answer in the terminal" icon={<TerminalWindow size={16} color={t.text} />} onPress={openTerminal} />
        ) : (
          <View style={styles.buttons}>
            <Button
              kind="primary"
              capsule
              title="Allow once"
              mark={markOf(Answer_Choice.ALLOW_ONCE, t.onAccent)}
              disabled={done}
              onPress={() => void answer(Answer_Choice.ALLOW_ONCE)}
              style={styles.first}
            />
            {question.always ? (
              <Button
                capsule
                title="Always"
                mark={markOf(Answer_Choice.ALLOW_ALWAYS, t.text)}
                disabled={done}
                onPress={() => void answer(Answer_Choice.ALLOW_ALWAYS)}
                style={styles.grow}
              />
            ) : null}
            <Button
              kind="danger"
              capsule
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
    <RuledCard>
      <Row
        ruled
        chevron
        mark={<AgentTile agent={agent} session={colorOf(worktree)} tone={t.attention} label={`${agent ? agentLabel(agent) : 'Agent'}, waiting on you`} />}
        title={worktree.name}
        titleColor={sessionColor(colorOf(worktree))?.label}
        detail={[many ? hostState.host.name : null, project.name, agent ? agentLabel(agent) : null].filter(Boolean).join(' · ')}
        onPress={openChat}
      />
      <View style={styles.body}>
        {body}
        {why ? <Text style={styles.why}>{why}</Text> : null}
      </View>
    </RuledCard>
  );
}

/** What the prompt is, in the waiting colour: "Wants to run", "Asks". */
function Asks({ children }: { children: ReactNode }) {
  return (
    <Text style={styles.asks} numberOfLines={2}>
      {children}
    </Text>
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
    <TouchableOpacity
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled, selected: picked }}
      activeOpacity={PRESSED}
      style={[styles.option, picked && { backgroundColor: t.selection }, disabled && !picked && { opacity: 0.55 }]}
    >
      <View style={styles.optionText}>
        <Text style={styles.optionLabel}>{label}</Text>
        {description ? <Text style={styles.detail}>{description}</Text> : null}
      </View>
      {picked ? <Check size={18} color={t.text} weight={2.4} /> : null}
    </TouchableOpacity>
  );
}

const styles = StyleSheet.create({
  // Under the head's rule: what it asks, and its answers.
  body: { paddingTop: 12, paddingHorizontal: 14, paddingBottom: 14, gap: 10 },
  detail: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  asksLine: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 8 },
  asks: { flexShrink: 1, fontFamily: font.proseSemibold, fontSize: size.label, color: t.attention },
  well: { ...surface.well, borderRadius: size.radiusLg, paddingVertical: 11, paddingHorizontal: 12 },
  code: { fontFamily: font.code, fontSize: 12.5, lineHeight: 18, color: t.text },
  lead: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  question: { fontFamily: font.proseSemibold, fontSize: size.title, lineHeight: 21, color: t.text },
  buttons: { flexDirection: 'row', gap: 8 },
  grow: { flex: 1 },
  // Allow once, a little wider than the answers beside it.
  first: { flex: 1.3 },
  options: { gap: 6 },
  option: {
    minHeight: 52,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 8,
    paddingHorizontal: 14,
    borderRadius: size.radiusLg,
    backgroundColor: t.elevated,
  },
  optionText: { flex: 1, minWidth: 0, gap: 2 },
  optionLabel: { fontFamily: font.proseMedium, fontSize: size.body, lineHeight: 18, color: t.text },
  more: { fontFamily: font.prose, fontSize: size.label, color: t.dim, textAlign: 'center' },
  why: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.failed },
});
