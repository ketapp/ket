// The rows of a chat that are not turns read from the transcript: a message
// of yours that the conversation has not shown yet, and the agent's working
// row. Drawn by the session's chat, and by the chat that stands in for one
// while the desktop is still making its worktree — see app/starting.

import { useEffect, useState } from 'react';
import { Pressable, StyleSheet, Text, View, type ViewStyle } from 'react-native';
import Svg, { Circle, Path } from 'react-native-svg';

import type { Attachment } from '../lib/attach';
import { font, size, t, tint } from '../lib/theme';
import { SentFiles } from './Attachments';
import { AgentMark } from './icons';

/** Where a message of yours stands. `held`: the desktop has it, and the
 * agent it is for is not running yet. */
export type OutgoingStatus = 'sending' | 'queued' | 'unseen' | 'held';

/** A message of yours on its way. */
export function Outgoing({
  text,
  files = [],
  status,
  onCopy,
  onDismiss,
}: {
  text: string;
  /** Photos and files sent with it, drawn above its words. */
  files?: Attachment[];
  status: OutgoingStatus;
  onCopy: (text: string) => void;
  onDismiss?: () => void;
}) {
  const said =
    status === 'queued'
      ? 'Queued, read when the agent finishes this step'
      : status === 'unseen'
        ? 'Not in the conversation, check the terminal'
        : status === 'held'
          ? 'Waiting on your Mac'
          : 'Sending';
  const faded = status === 'sending' || status === 'held';
  const words = <Text style={[styles.youText, faded && styles.youSendingText]}>{text}</Text>;
  const what = text || (files.length === 1 ? files[0].name : `${files.length} files`);
  return (
    <Pressable
      onPress={status === 'unseen' ? onDismiss : undefined}
      delayLongPress={350}
      onLongPress={() => onCopy(text)}
      accessibilityLabel={`${what}. ${said}.`}
      accessibilityHint={status === 'unseen' ? 'Tap to clear, hold to copy' : 'Hold to copy'}
      style={({ pressed }) => [
        files.length > 0 ? styles.yours : [styles.you, faded && styles.youSending],
        pressed && styles.held,
      ]}
    >
      {files.length > 0 ? (
        <>
          <SentFiles files={files} sending={faded} />
          {text ? (
            <View style={[styles.bubble, faded && styles.youSending]}>
              <StatusMark status={status} />
              {words}
            </View>
          ) : (
            <StatusMark status={status} />
          )}
        </>
      ) : (
        <>
          <StatusMark status={status} />
          {words}
        </>
      )}
    </Pressable>
  );
}

/**
 * Where a message of yours stands, as a mark in the gutter to the left of its
 * bubble: a dashed ring while it is sent, an amber clock while the agent — or
 * the desktop, for a message to an agent not running yet — holds it, a red !
 * when it never arrived, a faint tick once it is in.
 *
 * Centred on the bubble's first line, however many lines the message runs to,
 * and drawn outside the bubble's box: it never shifts the text, never changes
 * where a long message wraps, and changing from one mark to the next — or the
 * tick moving on to the next message — moves nothing else. Shape as well as
 * colour tells the marks apart.
 */
export function StatusMark({ status }: { status: OutgoingStatus | 'delivered' }) {
  return (
    <View pointerEvents="none" style={styles.statusMark}>
      <Svg width={14} height={14} viewBox="0 0 14 14">
        {status === 'sending' ? (
          <Circle cx={7} cy={7} r={5.25} fill="none" stroke={t.faint} strokeWidth={1.5} strokeDasharray="3 2.5" />
        ) : status === 'queued' || status === 'held' ? (
          <>
            <Circle cx={7} cy={7} r={5.25} fill="none" stroke={t.quotaWarm} strokeWidth={1.5} />
            <Path d="M7 4.2V7l2 1.3" fill="none" stroke={t.quotaWarm} strokeWidth={1.5} strokeLinecap="round" />
          </>
        ) : status === 'unseen' ? (
          <>
            <Circle cx={7} cy={7} r={6} fill={t.failed} />
            <Path d="M7 3.8v3.8" stroke={t.backdrop} strokeWidth={1.6} strokeLinecap="round" />
            <Circle cx={7} cy={9.9} r={0.95} fill={t.backdrop} />
          </>
        ) : (
          <Path
            d="M3.5 7.3 5.9 9.6 10.5 4.6"
            fill="none"
            stroke={t.faint}
            strokeWidth={1.6}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        )}
      </Svg>
    </View>
  );
}

/** The agent at work: its mark, three dots taking turns, and what it is
 * doing — or what the desktop is doing to get it running. */
export function Working({ agent, what }: { agent: string; what: string }) {
  const [phase, setPhase] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setPhase((n) => (n + 1) % 3), 450);
    return () => clearInterval(timer);
  }, []);
  return (
    <View style={styles.agent} accessibilityLabel={what || 'Waiting for the agent'}>
      {agent ? (
        <View style={styles.agentMark}>
          <AgentMark agent={agent} size={14} />
        </View>
      ) : null}
      <View style={styles.working}>
        {[0, 1, 2].map((at) => (
          <View key={at} style={[styles.workingDot, at === phase && { backgroundColor: t.running }]} />
        ))}
        {/* Always laid out, so the words arriving do not change its height. */}
        <Text style={styles.workingText} numberOfLines={1}>
          {what || ' '}
        </Text>
      </View>
    </View>
  );
}

/** A message of yours: a bubble on the right, its tail at the foot. */
export const bubble: ViewStyle = {
  backgroundColor: t.selection,
  borderRadius: 18,
  borderBottomRightRadius: 6,
  paddingHorizontal: 12,
  paddingVertical: 9,
  gap: 6,
};

const styles = StyleSheet.create({
  you: { ...bubble, alignSelf: 'flex-end', maxWidth: '86%' },
  // A message of yours with photos or files: them, then its bubble.
  yours: { alignSelf: 'flex-end', maxWidth: '86%', alignItems: 'flex-end', gap: 4 },
  bubble,
  // Faded while sending — the fill and the words, not the mark beside them —
  // at the delivered bubble's size and corners.
  youSending: { backgroundColor: tint(t.selection, 0.7) },
  youSendingText: { opacity: 0.7 },
  // In the gutter left of the bubble, centred on the first line: the bubble's
  // top padding, then half of a 20pt line less half the 14pt mark.
  statusMark: { position: 'absolute', left: -20, top: 12 },
  youText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 20, color: t.text },
  held: { opacity: 0.7 },
  agent: { flexDirection: 'row', gap: 10, alignItems: 'flex-start' },
  // Centred on the first line of the words beside it — the body's 1pt top
  // padding plus a 21pt line — however many lines they run to.
  agentMark: { height: 22, paddingTop: 1, justifyContent: 'center' },
  // As tall as a first line of words, so the mark sits beside it as it does beside them.
  working: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: 5, paddingTop: 1, minHeight: 22 },
  workingDot: { width: 5, height: 5, borderRadius: 3, backgroundColor: t.faint },
  workingText: { flex: 1, minWidth: 0, marginLeft: 4, fontFamily: font.chrome, fontSize: size.detail, color: t.dim },
});
