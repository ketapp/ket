// A session its plan stopped (protocol 1.38): the chat's composer gives way
// to a warm line saying so, and a warm sheet over the chat says when the
// limit resets and offers the agent ket would hand the task to. Both are
// the shared pieces — `Sheet` in its `warn` tone, `Button` in its `warn`
// kind — so the stop reads as one of the app's own sheets.
//
// The sheet comes up once a stop: closed, it stays closed for that stop,
// and the line in the composer's place brings it back.

import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { type SessionLimit, SwitchDepth, type SwitchPreview } from '@ket/remote';

import { clockOf, closedSheets as closed, isOver, stopSentence, waited } from '../lib/limits';
import { reasonOf } from '../lib/text';
import { useNow } from '../lib/usage';
import { agentLabel, blend, font, size, t } from '../lib/theme';
import { AgentMark } from './icons';
import { Sheet, SheetItem, SheetList, SheetNote, warm } from './Sheet';
import { Button, Meter, ON_WARN, Segmented } from './ui';


/** Whether the sheet for the stop named `key` is up, which agent it is
 * handing to when it has unfolded, and the ways to change both. An empty
 * key is no stop. */
export function useLimitSheet(key: string): {
  open: boolean;
  to: string | undefined;
  close: () => void;
  reopen: () => void;
  handTo: (agent: string | undefined) => void;
} {
  const [, redraw] = useState(0);
  const [to, setTo] = useState<string | undefined>(undefined);
  return {
    open: key !== '' && !closed.has(key),
    to,
    close: () => {
      setTo(undefined);
      if (key === '' || closed.has(key)) return;
      closed.add(key);
      redraw((n) => n + 1);
    },
    reopen: () => {
      if (!closed.delete(key)) return;
      redraw((n) => n + 1);
    },
    handTo: (agent) => {
      closed.delete(key);
      setTo(agent);
      redraw((n) => n + 1);
    },
  };
}

/** `limit` while it still stops the session; nothing once it has reset. A
 * screen showing it is drawn again the moment it resets, not only at its
 * next snapshot: the limit stays on the session until it gets going. */
export function useStopped(limit: SessionLimit | undefined): SessionLimit | undefined {
  const resets = limit ? Number(limit.resetsAtMs) : 0;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!resets) return;
    const left = Math.max(0, resets - Date.now());
    // Past a timer's reach — a weekly limit — the next snapshot will do.
    if (left > 2_000_000_000) return;
    const timer = setTimeout(() => setNow(Date.now()), left + 250);
    return () => clearTimeout(timer);
  }, [resets]);
  return limit && !isOver(limit, now) ? limit : undefined;
}

/** The agent's mark on a dark tile, for a button in the quota's colour,
 * where its own colour would not stand off the fill. */
function Handover({ agent, small }: { agent: string; small?: boolean }) {
  const side = small ? 26 : 28;
  return (
    <View style={[styles.tile, { width: side, height: side, borderRadius: small ? side / 2 : 8 }]}>
      <AgentMark agent={agent} size={small ? 14 : 16} />
    </View>
  );
}

/** How a desktop hands the task over, for a sheet on one that can (1.39). */
export interface HandOver {
  /** The agents that could take over, in the desktop's order — see `handChoices`. */
  choices: { agent: string; spent: boolean; low: boolean }[];
  /** What `agent` would be told, at `depth`. */
  preview: (agent: string, depth: SwitchDepth) => Promise<SwitchPreview>;
  /** Hands it over with `text`, read from `session`; why not, or nothing
   * once it is done. */
  send: (agent: string, text: string, beside: boolean, session: string) => Promise<string | undefined>;
}

type Told = 'task' | 'recent' | 'full';
const DEPTH: Record<Told, SwitchDepth> = { task: SwitchDepth.TASK, recent: SwitchDepth.RECENT, full: SwitchDepth.FULL };

export function LimitSheet({
  open,
  onClose,
  agent,
  limit,
  use,
  hand,
  to,
  onTo,
}: {
  open: boolean;
  onClose: () => void;
  /** The agent that stopped. */
  agent: string;
  limit: SessionLimit;
  /** The agent to offer in its place; none when there is none to offer. */
  use?: string;
  /** How to hand it over; none from a desktop that cannot. */
  hand?: HandOver;
  /** The agent being handed to, once "Use" has unfolded the sheet. */
  to?: string;
  onTo: (agent: string) => void;
}) {
  const now = useNow();
  const who = agent ? agentLabel(agent) : 'The agent';
  const resets = Number(limit.resetsAtMs);
  const close = onClose;
  // "Use Claude" unfolds this same sheet into what Claude would be told,
  // and the choice of agent, depth and tab, before anything is sent.
  if (to && hand) {
    return (
      <Sheet
        open={open}
        onClose={close}
        title="Continue with…"
        detail={resets > now ? `${who} is out of usage until ${clockOf(resets)}` : `${who} is out of usage`}
        tone="warn"
      >
        <Handing hand={hand} to={to} onTo={onTo} />
      </Sheet>
    );
  }
  return (
    <Sheet open={open} onClose={close} title="Out of usage" tone="warn">
      <View style={styles.say}>
        <Text style={styles.sentence}>{stopSentence(who, limit, now)}</Text>
        {resets > now ? (
          <View style={styles.wait}>
            <Meter fraction={waited(limit, now)} color={t.quotaHot} style={[styles.track, warm.well]} />
            <Text style={styles.clock}>{clockOf(resets)}</Text>
          </View>
        ) : null}
      </View>
      <View style={styles.buttons}>
        <Button kind="secondary" title="Wait for it" onPress={close} style={[styles.half, warm.edge]} />
        {use && hand ? (
          <Button
            kind="warn"
            title={`Use ${agentLabel(use)}`}
            icon={<Handover agent={use} />}
            onPress={() => onTo(use)}
            style={styles.half}
          />
        ) : null}
      </View>
    </Sheet>
  );
}

/** The sheet unfolded: who takes over, what they are told — shown word for
 * word, since it goes to another company's model — and where. */
function Handing({ hand, to, onTo }: { hand: HandOver; to: string; onTo: (agent: string) => void }) {
  const [told, setTold] = useState<Told>('recent');
  const [beside, setBeside] = useState(false);
  const [preview, setPreview] = useState<SwitchPreview | undefined>(undefined);
  const [why, setWhy] = useState<string | undefined>(undefined);
  const [sending, setSending] = useState(false);
  // The brief for what is chosen now: an older answer arriving late is
  // not shown over it. Asked again only when the choice changes — `hand`
  // is new with every snapshot, and each read spends the desktop's budget.
  const asked = useRef(0);
  const previewOf = useEffectEvent((agent: string, depth: SwitchDepth) => hand.preview(agent, depth));
  useEffect(() => {
    const ask = ++asked.current;
    previewOf(to, DEPTH[told])
      .then((brief) => {
        if (ask !== asked.current) return;
        setPreview(brief);
        setWhy(undefined);
      })
      .catch((error: unknown) => {
        if (ask !== asked.current) return;
        setPreview(undefined);
        setWhy(reasonOf(error));
      });
  }, [to, told]);
  const shown = preview;
  const send = async () => {
    if (!shown || sending) return;
    setSending(true);
    const failed = await hand.send(to, shown.text, beside, shown.session);
    setSending(false);
    setWhy(failed);
  };
  const name = agentLabel(to);
  return (
    <>
      <SheetList>
        {hand.choices.map((choice) => (
          <SheetItem
            key={choice.agent}
            label={agentLabel(choice.agent)}
            mark={<AgentMark agent={choice.agent} size={16} />}
            detail={choice.spent ? 'Out of usage too' : choice.low ? 'Nearly out of usage' : undefined}
            dot={choice.spent ? t.quotaHot : choice.low ? t.quotaWarm : undefined}
            on={choice.agent === to}
            held={sending}
            role="radio"
            onPress={() => {
              setPreview(undefined);
              onTo(choice.agent);
            }}
          />
        ))}
      </SheetList>
      <View style={styles.part}>
        <Text style={[styles.partLabel, warm.detail]}>What it’s told</Text>
        <Segmented
          segments={[
            { key: 'task', label: 'The task' },
            { key: 'recent', label: 'Recent' },
            { key: 'full', label: 'Everything' },
          ]}
          value={told}
          onChange={(key) => {
            setPreview(undefined);
            setTold(key);
          }}
          disabled={sending}
        />
      </View>
      <View style={styles.part}>
        <View style={styles.partHead}>
          <Text style={[styles.partLabel, warm.detail]}>{`${name} will be sent this`}</Text>
          {shown ? (
            <Text style={[styles.partLabel, warm.detail]}>{`about ${shown.approxTokens.toLocaleString()} tokens`}</Text>
          ) : null}
        </View>
        <ScrollView style={[styles.preview, warm.well]} nestedScrollEnabled>
          <Text selectable style={styles.previewText}>
            {shown ? shown.text : why ? '' : 'Reading the conversation…'}
          </Text>
        </ScrollView>
      </View>
      <Segmented
        segments={[
          { key: 'here', label: 'In this chat' },
          { key: 'beside', label: 'In a new chat beside it' },
        ]}
        value={beside ? 'beside' : 'here'}
        onChange={(key) => setBeside(key === 'beside')}
        disabled={sending}
      />
      {why ? <SheetNote>{why}</SheetNote> : null}
      <Button
        kind="warn"
        title={sending ? `Starting ${name}…` : `Send to ${name}`}
        onPress={() => void send()}
        disabled={!shown || sending}
        style={styles.send}
      />
    </>
  );
}

/** In the composer's place while the sheet is closed: what stopped, a tap
 * from the sheet, and the agent to use. */
export function LimitBar({
  limit,
  onOpen,
  use,
  onUse,
}: {
  limit: SessionLimit;
  onOpen: () => void;
  use?: string;
  onUse?: () => void;
}) {
  const now = useNow();
  const resets = Number(limit.resetsAtMs);
  const when = resets > now ? `Resets in ${shortWait(resets - now)}` : 'Waiting for its limit to reset';
  return (
    <View style={[styles.bar, warm.sheet]}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Usage limit reached. ${when}. Show what you can do`}
        onPress={onOpen}
        style={styles.barWords}
      >
        <Text style={[styles.barLabel, warm.title]} numberOfLines={1}>
          Usage limit reached
        </Text>
        <Text style={styles.barWhen} numberOfLines={1}>
          {when}
        </Text>
      </Pressable>
      {use && onUse ? (
        <Button
          kind="warn"
          title={`Use ${agentLabel(use)}`}
          icon={<Handover agent={use} small />}
          onPress={onUse}
          style={styles.barUse}
        />
      ) : null}
    </View>
  );
}

/** `2 h 6 min`: the bar's room for the wait. */
function shortWait(ms: number): string {
  const minutes = Math.max(1, Math.ceil(ms / 60_000));
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return `${days.toString()} d ${hours.toString()} h`;
  if (hours > 0) return `${hours.toString()} h ${mins.toString()} min`;
  return `${mins.toString()} min`;
}

const styles = StyleSheet.create({
  say: { gap: 12 },
  sentence: { fontFamily: font.proseSemibold, fontSize: 19, lineHeight: 25, color: t.text },
  wait: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  track: { flex: 1 },
  clock: { fontFamily: font.prose, fontSize: 13, fontVariant: ['tabular-nums'], color: blend(t.text, t.quotaHot, 0.2) },
  buttons: { flexDirection: 'row', gap: 10, paddingTop: 4 },
  // Half each, whatever their words.
  half: { flexBasis: 0, flexGrow: 1, minWidth: 0, height: 56, borderRadius: 14 },
  tile: { backgroundColor: ON_WARN, alignItems: 'center', justifyContent: 'center' },
  bar: {
    marginHorizontal: size.gutter,
    height: 56,
    borderRadius: 28,
    borderWidth: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingLeft: 18,
    paddingRight: 6,
  },
  barWords: { flex: 1, minWidth: 0, height: 44, justifyContent: 'center', gap: 1 },
  barLabel: { fontFamily: font.proseMedium, fontSize: 12, lineHeight: 15 },
  barWhen: { fontFamily: font.proseSemibold, fontSize: 15, lineHeight: 19, color: t.text },
  barUse: { height: 44, borderRadius: 22, paddingLeft: 8, paddingRight: 14 },
  part: { gap: 7 },
  partHead: { flexDirection: 'row', justifyContent: 'space-between', gap: 10 },
  partLabel: { fontFamily: font.proseMedium, fontSize: 12.5 },
  preview: { maxHeight: 150, borderRadius: 10, paddingHorizontal: 12, paddingVertical: 10 },
  previewText: { fontFamily: font.prose, fontSize: 13, lineHeight: 19, color: blend(t.text, t.quotaHot, 0.12) },
  send: { height: 52, borderRadius: 14 },
});
