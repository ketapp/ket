// The cards that float over the conversation's foot while something waits on
// you, as the chat draws them (see app/session/[host]/[terminal].tsx): their
// shared look, and the one for a question the agent asks with options or a
// plan it wants to go ahead with.

import { type Question } from '@ket/remote';
import { useState } from 'react';
import { ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';

import { Dim } from './Tuck';
import { Check, Send, TerminalWindow } from './icons';
import { Button, Choice, field, oneLine } from './ui';
import { type PromptAnswer } from '../lib/connection';
import { blend, font, size, t } from '../lib/theme';

/** A card over the conversation's foot: a question, a plan, a permission,
 * a cold session. */
export const cardStyles = StyleSheet.create({
  ask: {
    marginHorizontal: size.gutter,
    marginBottom: 8,
    padding: 12,
    gap: 8,
    borderRadius: size.radiusCard,
    // Solid: it rides over the conversation with the keyboard, and the
    // conversation must not read through it.
    backgroundColor: blend(t.backdrop, t.attention, 0.1),
    borderWidth: 1,
    borderColor: blend(t.backdrop, t.attention, 0.35),
  },
  askTitle: { fontFamily: font.proseSemibold, fontSize: size.body, color: t.text },
  askSubject: { fontFamily: font.code, fontSize: 12.5, lineHeight: 18, color: t.text },
  askLead: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  askButtons: { flexDirection: 'row', gap: 8 },
  askBody: { gap: 8 },
  grow: { flex: 1 },
});

/** A question the agent asks with options, or a plan it wants to go ahead
 * with: answered here and handed to the agent by the desktop. When the
 * desktop cannot take the answer — an older ket, or a wait that ran out —
 * it says so and opens the terminal instead. */
export function Asking({
  question,
  kind,
  busy,
  onRespond,
  onTerminal,
}: {
  question: Question;
  kind: 'ask' | 'plan';
  busy: boolean;
  onRespond: (reply: PromptAnswer) => Promise<boolean>;
  onTerminal: () => void;
}) {
  const [picks, setPicks] = useState<number[][]>(() => question.asks.map(() => []));
  const [others, setOthers] = useState<string[]>(() => question.asks.map(() => ''));
  const [refining, setRefining] = useState(false);
  const [feedback, setFeedback] = useState('');
  // Answered: held until the desktop's next snapshot takes the card away.
  const [sent, setSent] = useState(false);
  const idle = !busy && !sent;
  const send = async (reply: PromptAnswer) => {
    if (await onRespond(reply)) setSent(true);
  };
  // Sent: the button that sent it wears a tick until the card goes.
  const sentMark = sent ? <Check size={20} color={t.onAccent} weight={2.4} /> : undefined;

  if (!question.answerable || (kind === 'ask' && question.asks.length === 0)) {
    return (
      <View style={cardStyles.ask}>
        <Text style={cardStyles.askTitle} numberOfLines={1}>
          {kind === 'plan' ? 'Has a plan ready' : 'Has a question'}
        </Text>
        {question.subject ? (
          <Text style={cardStyles.askLead} numberOfLines={3}>
            {question.subject}
          </Text>
        ) : null}
        <Button
          kind="primary"
          title="Answer in the terminal"
          icon={<TerminalWindow size={16} color={t.onAccent} />}
          onPress={onTerminal}
        />
      </View>
    );
  }

  if (kind === 'plan') {
    return (
      <View style={cardStyles.ask}>
        <Dim on={sent} style={cardStyles.askBody}>
          <Text style={cardStyles.askTitle}>Ready to code?</Text>
          {question.subject ? (
            <Text style={cardStyles.askLead} numberOfLines={2}>
              {question.subject}
            </Text>
          ) : null}
          {refining ? (
            <TextInput
              value={feedback}
              onChangeText={setFeedback}
              placeholder="Tell Claude what to change"
              placeholderTextColor={t.faint}
              multiline
              autoFocus
              editable={idle}
              style={[field, styles.askInput]}
            />
          ) : null}
        </Dim>
        {refining ? (
          <View style={cardStyles.askButtons}>
            <Button
              kind="primary"
              title="Send"
              mark={sentMark}
              disabled={!idle || !feedback.trim()}
              onPress={() => void send({ approve: false, feedback: feedback.trim() })}
              style={cardStyles.grow}
            />
            <Button kind="ghost" title="Cancel" disabled={!idle} onPress={() => setRefining(false)} style={cardStyles.grow} />
          </View>
        ) : (
          <View style={cardStyles.askButtons}>
            <Button
              kind="primary"
              title="Approve"
              mark={sentMark}
              disabled={!idle}
              onPress={() => void send({ approve: true })}
              style={cardStyles.grow}
            />
            <Button title="Keep planning" disabled={!idle} onPress={() => setRefining(true)} style={cardStyles.grow} />
          </View>
        )}
      </View>
    );
  }

  const answered = question.asks.every((_, i) => picks[i].length > 0 || others[i].trim() !== '');
  // A choice and words of your own are one answer or the other.
  const choose = (i: number, j: number, many: boolean) => {
    setPicks((before) =>
      before.map((p, k) => (k !== i ? p : !many ? [j] : p.includes(j) ? p.filter((x) => x !== j) : [...p, j])),
    );
    setOthers((before) => before.map((o, k) => (k === i ? '' : o)));
  };
  const write = (i: number, text: string) => {
    setOthers((before) => before.map((o, k) => (k === i ? text : o)));
    if (text.trim()) setPicks((before) => before.map((p, k) => (k === i ? [] : p)));
  };
  return (
    <View style={cardStyles.ask}>
      <Dim on={sent}>
        <ScrollView style={styles.askScroll} contentContainerStyle={styles.askQuestions} keyboardShouldPersistTaps="handled">
          {question.asks.map((asked, i) => (
            <View key={i} style={styles.asked}>
              {asked.header ? <Text style={styles.askHeader}>{asked.header}</Text> : null}
              <Text style={cardStyles.askTitle}>{asked.question}</Text>
              {asked.multiSelect ? <Text style={cardStyles.askLead}>Pick any</Text> : null}
              <View>
                {asked.choices.map((choice, j) => (
                  <Choice
                    key={j}
                    many={asked.multiSelect}
                    on={picks[i].includes(j)}
                    label={choice.label}
                    description={choice.description}
                    disabled={!idle}
                    onPress={() => choose(i, j, asked.multiSelect)}
                  />
                ))}
              </View>
              {question.tool === 'Dialog' || question.tool === 'request_user_input' ? null : (
                // A dialog read off the agent's screen, or Codex's form, takes
                // one of its own options and nothing else: the desktop types
                // the option's number.
                <TextInput
                  value={others[i]}
                  onChangeText={(text) => write(i, text)}
                  placeholder="Or write your own"
                  placeholderTextColor={t.faint}
                  {...oneLine}
                  editable={idle}
                  style={field}
                />
              )}
            </View>
          ))}
        </ScrollView>
      </Dim>
      <View style={cardStyles.askButtons}>
        <Button
          kind="primary"
          title={question.asks.length > 1 ? 'Send answers' : 'Send answer'}
          mark={sentMark}
          icon={<Send size={16} color={t.onAccent} weight={2.3} />}
          disabled={!idle || !answered}
          onPress={() =>
            void send({ picks: question.asks.map((_, i) => ({ choices: picks[i], other: others[i].trim() })) })
          }
          style={cardStyles.grow}
        />
        <Button
          title="In the terminal"
          icon={<TerminalWindow size={16} color={t.text} />}
          onPress={onTerminal}
          style={cardStyles.grow}
        />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  askHeader: { fontFamily: font.chrome, fontSize: size.caption, color: t.attention },
  askScroll: { flexGrow: 0, maxHeight: 380 },
  askQuestions: { gap: 16 },
  asked: { gap: 6 },
  askInput: { height: undefined, minHeight: size.control, maxHeight: 120, paddingTop: 11, paddingBottom: 11, lineHeight: 20 },
});
