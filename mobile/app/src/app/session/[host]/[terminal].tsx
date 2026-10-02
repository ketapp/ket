'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// An agent's session as a conversation: what you said, what it said back,
// and each tool it used as one line — read from its transcript on the
// desktop, not drawn from a terminal screen. A message line under it types
// into the agent's terminal; when the agent is asking to be allowed
// something, the answer is right here. The terminal itself is one tap away.

import { Activity, Answer_Choice, type Question, type Turn, Turn_Kind } from '@ket/remote';
import * as Clipboard from 'expo-clipboard';
import { router, useFocusEffect, useLocalSearchParams } from 'expo-router';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { AppState, Linking, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { KeyboardChatScrollView, KeyboardStickyView } from 'react-native-keyboard-controller';
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import Svg, { Circle, Defs, LinearGradient, Path, Rect, Stop as GradientStop } from 'react-native-svg';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { CommandMenu } from '../../../components/Commands';
import { RiskBadge } from '../../../components/Risk';
import { AgentMark, Bookmark, Branch, Check, ChevronDown, Send, Stop, TerminalGlyph } from '../../../components/icons';
import { Bar, Button, Choice, Dot, IconButton, Prose, Screen, field } from '../../../components/ui';
import { detail, everyWorktree, stateOf } from '../../../lib/activity';
import { type Command, commandsFor, matching } from '../../../lib/commands';
import type { PromptAnswer } from '../../../lib/connection';
import { useHost } from '../../../lib/hosts';
import { confirmIdentity } from '../../../lib/lock';
import { agentLabel, font, size, t, tint } from '../../../lib/theme';

/** How often the conversation is asked for while it is on screen: more
 * often while the agent is working or a message is on its way. */
const POLL_MS = 1500;
const POLL_BUSY_MS = 700;

/** The pause between a message and its Enter — see the terminal screen. */
const ENTER_AFTER_MS = 80;

/** How long a sent message may go unseen — not in the conversation, not
 * waiting in the agent's queue — before it is shown as not delivered. */
const UNSEEN_MS = 15_000;

/** How long after sending the composer turns away what iOS puts back:
 * Send tapped mid-autocorrect, the correction lands after the clearing. */
const SETTLE_MS = 300;

/** A message sent from here that the conversation has not shown yet. */
type Pending = {
  key: number;
  text: string;
  at: number;
  /** Sent while the agent was working, so it waits in the agent's queue,
   * after the step in progress. Sent while it was idle, it is what starts
   * the next step, and belongs before the step's indicator. */
  behind: boolean;
};

/** How tall the fade over the conversation's foot is, and how long it
 * takes to come and go. */
const EDGE_H = 28;
const EDGE_MS = 180;

/** One-tap replies every session has. Chips over the composer while the
 * draft is empty, after `/` and the desktop's snippets. */
const REPLIES = ['Continue', 'Looks good', 'Run the tests', 'Commit it'];

/** What each conversation last showed, by host and terminal, while the app
 * runs: coming back to a chat draws it at once and reads on from where it
 * was, rather than starting over at "Loading the conversation…". */
const kept = new Map<string, { turns: Turn[]; cursor: bigint; session: string }>();

export default function Session() {
  const { host: hostId, terminal } = useLocalSearchParams<{ host: string; terminal: string }>();
  const terminalId = BigInt(terminal);
  const conn = useHost(hostId);
  const insets = useSafeAreaInsets();
  const keptKey = `${hostId}:${terminal}`;
  const [turns, setTurns] = useState<Turn[]>(() => kept.get(keptKey)?.turns ?? []);
  // Sent from here and not in the conversation yet: shown at once, as
  // sending, then as queued while the agent's queue holds it.
  const [pending, setPending] = useState<Pending[]>([]);
  const [queued, setQueued] = useState<string[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const sentCount = useRef(0);
  const [unreadable, setUnreadable] = useState('');
  // Whether the desktop has answered once, and why it has not when asking
  // keeps failing: an empty conversation, one still loading and one the
  // desktop is refusing looked the same — blank.
  const [loaded, setLoaded] = useState(() => kept.has(keptKey));
  const [failing, setFailing] = useState('');
  const [message, setMessage] = useState('');
  const [note, setNote] = useState('');
  const [answering, setAnswering] = useState(false);
  // A Stop just pressed: the agent has the interrupt, and a second press
  // before it has stopped would reach whatever comes after.
  const [stopping, setStopping] = useState(false);
  const [snippets, setSnippets] = useState<{ name: string; body: string }[]>([]);
  const cursor = useRef(kept.get(keptKey)?.cursor ?? 0n);
  const session = useRef(kept.get(keptKey)?.session ?? '');
  // Kept with the cursor that produced it: a later read that moved the cursor
  // without new turns only leaves the kept cursor behind, over records that
  // make no turns, so reading on from it never repeats one.
  useEffect(() => {
    if (turns.length > 0) kept.set(keptKey, { turns, cursor: cursor.current, session: session.current });
  }, [keptKey, turns]);
  const held = useRef(false);
  const scroll = useRef<Animated.ScrollView>(null);
  const input = useRef<TextInput>(null);
  const clearedAt = useRef(0);
  // Whether the view is at the newest turn, so new ones scroll it along —
  // and one scrolled up to read is left where it is.
  const atEnd = useRef(true);
  // Scrolled up from the newest turn, and whether new ones have come since:
  // the jump back down, and its dot.
  const [away, setAway] = useState(false);
  const [fresh, setFresh] = useState(false);

  const located = everyWorktree(conn?.snapshot ?? null)
    .map((l) => ({ ...l, terminal: l.worktree.terminals.find((x) => x.id === terminalId) }))
    .find((l) => l.terminal);
  const worktree = located?.worktree;
  const state = worktree ? stateOf(worktree.activity) : undefined;
  // Working, as the desktop's sidebar sees it: the agent thinks for a while
  // before anything reaches its transcript, and this says so meanwhile.
  const working = worktree?.activity === Activity.WORKING || worktree?.activity === Activity.RUNNING;
  const busy = useRef(false);
  const isBusy = working || pending.length > 0 || queued.length > 0;
  useEffect(() => {
    busy.current = isBusy;
  }, [isBusy]);
  const question =
    worktree?.activity === Activity.BLOCKED && worktree.question?.terminalId === terminalId ? worktree.question : undefined;
  // A question or a plan rather than a permission: its own card, answerable
  // or not, in place of the permission's.
  const prompt = question ? promptKind(question.tool) : undefined;
  const asking = !!question && (question.answerable || prompt !== undefined);

  // Asked for now and every little while; each answer carries on from the
  // last. A new session in the same terminal starts the list over.
  // A desktop from before 1.7 has no conversations to give, and ignores
  // the asking: said plainly rather than left blank.
  const tooOld = conn !== null && conn.status === 'connected' && conn.hostMinor < 7;

  // Only while this screen is the one showing. The terminal opens on top of
  // it, and a conversation polled out of sight spent the desktop's budget for
  // costly requests, so the terminal's own subscribe was refused and it stayed
  // blank. Coming back asks at once and carries on from the cursor.
  useFocusEffect(
    useCallback(() => {
      if (!conn || tooOld) return;
      let stopped = false;
      // One question at a time: a slow answer is not asked for again on top.
      let asking = false;
      // Failures in a row; a few say so on screen, one does not.
      let failures = 0;
      const tick = async () => {
        if (asking) return;
        asking = true;
        try {
          const reply = await conn.conversation(terminalId, cursor.current);
          if (stopped) return;
          failures = 0;
          setLoaded(true);
          setFailing('');
          if (!reply.supported) {
            setUnreadable(reply.reason);
            return;
          }
          setUnreadable('');
          const restart = reply.fresh || reply.session !== session.current;
          session.current = reply.session;
          cursor.current = reply.next;
          if (restart) setTurns(reply.turns);
          else if (reply.turns.length > 0) {
            setTurns((before) => join(before, reply.turns));
            if (!atEnd.current) setFresh(true);
          }
          setQueued(reply.queued);
          // A message that has reached the conversation is no longer pending.
          // After a fresh read, only the newest few can be one just sent.
          const arrived = (restart ? reply.turns.slice(-6) : reply.turns)
            .filter((turn) => turn.kind === Turn_Kind.USER)
            .map((turn) => turn.text);
          if (arrived.length > 0) {
            setPending((before) => {
              const left = [...arrived];
              return before.filter((p) => {
                const at = left.findIndex((said) => carries(said, p.text));
                if (at < 0) return true;
                left.splice(at, 1);
                return false;
              });
            });
          }
          setNow(Date.now());
        } catch (e) {
          // Offline for a moment, or the desktop busy: the next tick asks
          // again, and a run of failures says why rather than staying blank.
          failures += 1;
          if (!stopped && failures >= 3) setFailing(e instanceof Error ? e.message : String(e));
        } finally {
          asking = false;
        }
      };
      // One timer at a time: every path here clears it before setting it.
      let timer: ReturnType<typeof setTimeout> | undefined;
      const loop = async () => {
        if (stopped) return;
        await tick();
        // A tick already under way schedules the next one when it ends.
        if (stopped || asking) return;
        clearTimeout(timer);
        timer = setTimeout(() => void loop(), busy.current ? POLL_BUSY_MS : POLL_MS);
      };
      // Asked now rather than at the next tick: the connection is back, or
      // the app is — opened from the lock screen on this very chat.
      const now = () => {
        clearTimeout(timer);
        void loop();
      };
      let wasConnected = conn.status === 'connected';
      const offChange = conn.onChange(() => {
        const connected = conn.status === 'connected';
        if (connected && !wasConnected) now();
        wasConnected = connected;
      });
      const appState = AppState.addEventListener('change', (next) => {
        if (next === 'active') now();
      });
      void loop();
      return () => {
        stopped = true;
        clearTimeout(timer);
        offChange();
        appState.remove();
      };
    }, [conn, terminalId, tooOld]),
  );

  // The desktop's saved snippets, once, as more chips over the composer.
  useEffect(() => {
    if (!conn || tooOld) return;
    let stopped = false;
    void conn
      .snippets()
      .then((list) => {
        if (!stopped) setSnippets(list.filter((s) => s.body.trim()));
      })
      .catch(() => {});
    return () => {
      stopped = true;
    };
  }, [conn, tooOld]);

  // Typing needs the terminal's control, taken the first time and kept.
  const type = async (text: string): Promise<boolean> => {
    if (!conn) return false;
    if (!held.current) {
      held.current = await conn.takeControl(terminalId).catch(() => false);
      if (!held.current) {
        setNote('Another device is typing in this terminal.');
        return false;
      }
    }
    try {
      if (await conn.type(terminalId, text)) return true;
      held.current = false;
      setNote('Your desktop took control back. Send again to take it.');
    } catch (e) {
      setNote(`Not sent: ${e instanceof Error ? e.message : String(e)}`);
    }
    return false;
  };

  // The text, then Enter on its own, as the terminal screen sends it: sent
  // together, an agent reads the Enter as a new line in a paste.
  const send = async (said?: string) => {
    const text = (said ?? message).trim();
    if (!text) return;
    if (said === undefined) {
      setMessage('');
      input.current?.clear();
      clearedAt.current = Date.now();
    }
    setNote('');
    const key = ++sentCount.current;
    setPending((before) => [...before, { key, text, at: Date.now(), behind: working }]);
    atEnd.current = true;
    if (!(await type(text))) {
      setPending((before) => before.filter((p) => p.key !== key));
      if (said === undefined) setMessage(text);
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, ENTER_AFTER_MS));
    await type('\r');
    atEnd.current = true;
  };

  // Held on a message or a code block: its words, on the clipboard.
  const copy = (text: string) => {
    void Clipboard.setStringAsync(text);
    setNote('Copied');
    setTimeout(() => setNote((shown) => (shown === 'Copied' ? '' : shown)), 1500);
  };

  // The turn stopped, as Escape stops it at the desktop; needs no control.
  const stop = () => {
    if (!conn || stopping) return;
    setStopping(true);
    conn.interrupt(terminalId);
    setTimeout(() => setStopping(false), 1500);
  };

  const answer = async (choice: Answer_Choice) => {
    if (!conn || !question) return;
    // "Always" outlives this prompt: with the app lock on, it is you first.
    if (choice === Answer_Choice.ALLOW_ALWAYS && !(await confirmIdentity(`Always allow ${question.tool}`))) return;
    setAnswering(true);
    const why = await conn.answer(question, choice).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setAnswering(false);
    if (why) setNote(why);
  };

  // A question's answers or a plan's verdict, handed to the agent by the
  // desktop. `true` once it has.
  const respond = async (reply: PromptAnswer): Promise<boolean> => {
    if (!conn || !question) return false;
    setAnswering(true);
    const why = await conn.respond(question, reply).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setAnswering(false);
    if (why) setNote(why);
    return !why;
  };

  useEffect(
    () => () => {
      if (conn && held.current) conn.releaseControl(terminalId);
    },
    [conn, terminalId],
  );

  const agent = located?.terminal?.agent ?? worktree?.agent ?? '';
  // Under the composer at rest: clear of the home indicator rather than the
  // whole safe area. With the keyboard up the composer rides on it, 6pt
  // above, so it and the conversation are lifted by the keyboard less this.
  const rest = Math.max(insets.bottom - 18, 6);
  const lift = rest - 6;
  const openTerminal = () => router.push(`/terminal/${hostId}/${terminal}`);

  // The `/` menu, while the draft is a slash and the start of a name.
  const commands = commandsFor(agent);
  const suggested = matching(commands, message);
  // The `/` chip's menu: every command, with nothing typed and the keyboard
  // left as it was. A draft that is a slash shows its own matches instead.
  const [listing, setListing] = useState(false);
  const menu = listing && !message ? commands : suggested;
  const pick = async (command: Command) => {
    setListing(false);
    if (command.args) {
      setMessage(`${command.name} `);
      input.current?.focus();
      return;
    }
    setMessage('');
    await send(command.name);
    // Its picker is answered where it is drawn.
    if (command.picker) openTerminal();
  };

  // The newest message of yours in the conversation: it carries the tick that
  // says it arrived, in the place its sending ring was — unless a newer one
  // is still on its way, sending or queued, whose mark is then the only one.
  let lastYours = -1;
  for (let at = turns.length - 1; at >= 0; at -= 1) {
    if (turns[at].kind === Turn_Kind.USER) {
      lastYours = at;
      break;
    }
  }

  // Sent from here and in neither the conversation nor the agent's queue yet.
  const unsent = pending.filter((p) => !queued.some((q) => carries(q, p.text)));
  const statusOf = (p: Pending) => (now - p.at > UNSEEN_MS && !working ? 'unseen' : 'sending');
  const outgoing = (p: Pending) => (
    <Outgoing
      key={p.key}
      text={p.text}
      status={statusOf(p)}
      onCopy={copy}
      onDismiss={() => setPending((before) => before.filter((x) => x.key !== p.key))}
    />
  );
  // The working row from the moment a message goes, its dots alone until the
  // agent says what it is doing: it is where the agent's answer will start,
  // and adding it a beat after the message is what made the chat jump.
  const sending = unsent.some((p) => !p.behind && statusOf(p) === 'sending');

  return (
    <Screen>
      <View style={styles.fill}>
        <Bar
          back
          title={worktree?.name ?? 'Session'}
          lead={
            state ? (
              <View style={styles.state}>
                <Dot size={6} color={state.color} />
                <Text style={[styles.stateText, { color: state.color }]}>{state.word}</Text>
              </View>
            ) : undefined
          }
          right={
            <View style={styles.barActions}>
              {worktree ? (
                <IconButton
                  label="The worktree"
                  onPress={() => router.push(`/worktree/${hostId}/${encodeURIComponent(worktree.id)}`)}
                >
                  <Branch size={18} color={t.text} />
                </IconButton>
              ) : null}
              <IconButton label="Open the terminal" onPress={openTerminal}>
                <TerminalGlyph size={19} color={t.text} />
              </IconButton>
            </View>
          }
        />
        <KeyboardChatScrollView
          ref={scroll}
          // Lifted with the keyboard, frame for frame, while the newest turn
          // is in view; someone scrolled up to read is left there.
          keyboardLiftBehavior="whenAtEnd"
          offset={lift}
          style={styles.fill}
          contentContainerStyle={styles.page}
          keyboardShouldPersistTaps="handled"
          onScroll={(e) => {
            const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
            const end = contentOffset.y + layoutMeasurement.height >= contentSize.height - 48;
            atEnd.current = end;
            if (end === away) setAway(!end);
            if (end && fresh) setFresh(false);
          }}
          scrollEventThrottle={100}
          onContentSizeChange={() => {
            if (atEnd.current) scroll.current?.scrollToEnd({ animated: false });
          }}
        >
          {tooOld ? (
            <View style={styles.empty}>
              <Prose>
                ket on {conn?.host.name ?? 'your Mac'} is older than this app. Update it — rebuild ket and start a new
                host — to read sessions here.
              </Prose>
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : unreadable && turns.length === 0 ? (
            <View style={styles.empty}>
              <Prose>{sentence(unreadable)}</Prose>
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : turns.length === 0 && failing ? (
            <View style={styles.empty}>
              <Prose>{`The desktop isn’t sending the conversation: ${failing}. Still trying.`}</Prose>
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : turns.length === 0 && !loaded ? (
            <View style={styles.empty}>
              <Prose>Loading the conversation…</Prose>
            </View>
          ) : turns.length === 0 && unsent.length === 0 && !working ? (
            <View style={styles.empty}>
              <Prose>Nothing in this session yet. What you and the agent say shows here.</Prose>
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : null}
          {turns.map((turn, index) => (
            <TurnView
              key={index}
              turn={turn}
              agent={agent}
              onCopy={copy}
              delivered={index === lastYours && unsent.length === 0 && queued.length === 0}
            />
          ))}
          {/* A message that starts the agent working goes before the
              indicator: the desktop says the agent is working a moment
              before its transcript has the message that set it off. */}
          {unsent.filter((p) => !p.behind).map(outgoing)}
          {working || sending ? <Working agent={agent} what={working && worktree ? detail(worktree) : ''} /> : null}
          {queued.map((text, index) => (
            <Outgoing key={`q${index}`} text={text} status="queued" onCopy={copy} />
          ))}
          {unsent.filter((p) => p.behind).map(outgoing)}
        </KeyboardChatScrollView>

        <KeyboardStickyView offset={{ closed: 0, opened: lift }}>
        <FadingEdge shown={away} />
        {away ? (
          // Floats over the conversation's foot, riding with the composer.
          <View pointerEvents="box-none" style={styles.jumpDock}>
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={fresh ? 'Jump to the newest — new messages' : 'Jump to the newest'}
              onPress={() => {
                atEnd.current = true;
                setAway(false);
                setFresh(false);
                scroll.current?.scrollToEnd({ animated: true });
              }}
              style={({ pressed }) => [styles.jump, pressed && { backgroundColor: t.selection }]}
            >
              <ChevronDown size={18} color={t.text} weight={2.2} />
              {fresh ? <View style={styles.jumpDot} /> : null}
            </Pressable>
          </View>
        ) : null}
        {question && prompt ? (
          <Asking
            key={String(question.id)}
            question={question}
            kind={prompt}
            busy={answering}
            onRespond={respond}
            onTerminal={openTerminal}
          />
        ) : question?.answerable ? (
          <View style={styles.ask}>
            <Text style={styles.askTitle} numberOfLines={1}>
              Wants to use {question.tool}
            </Text>
            <RiskBadge risk={question.risk} />
            {question.subject ? (
              <Text style={styles.askSubject} numberOfLines={4}>
                {question.tool === 'Bash' ? '$ ' : ''}
                {question.subject}
              </Text>
            ) : null}
            <View style={styles.askButtons}>
              <Button kind="primary" title="Allow once" disabled={answering} onPress={() => void answer(Answer_Choice.ALLOW_ONCE)} style={styles.grow} />
              {question.always ? (
                <Button title="Always" disabled={answering} onPress={() => void answer(Answer_Choice.ALLOW_ALWAYS)} style={styles.grow} />
              ) : null}
              <Button kind="danger" title="Deny" disabled={answering} onPress={() => void answer(Answer_Choice.DENY)} style={styles.grow} />
            </View>
          </View>
        ) : null}

        {note ? (
          <Prose style={styles.note} color={t.dim}>
            {note}
          </Prose>
        ) : null}
        {menu.length > 0 ? <CommandMenu commands={menu} onPick={(command) => void pick(command)} /> : null}
        {!tooOld && !asking && !message ? (
          <ScrollView
            horizontal
            showsHorizontalScrollIndicator={false}
            keyboardShouldPersistTaps="handled"
            style={styles.replies}
            contentContainerStyle={styles.repliesInner}
          >
            {commands.length > 0 ? (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Commands"
                accessibilityState={{ expanded: listing }}
                onPress={() => setListing(!listing)}
                style={({ pressed }) => [styles.reply, styles.slash, (listing || pressed) && { backgroundColor: t.selection }]}
              >
                <Text style={styles.slashText}>/</Text>
              </Pressable>
            ) : null}
            {snippets.map((snippet, index) => (
              <Pressable
                key={`s${index}`}
                accessibilityRole="button"
                accessibilityLabel={`Send ${snippet.name}`}
                accessibilityHint="Hold to edit it first"
                onPress={() => void send(snippet.body)}
                onLongPress={() => {
                  setMessage(snippet.body);
                  input.current?.focus();
                }}
                style={({ pressed }) => [styles.reply, styles.snippet, pressed && { backgroundColor: t.selection }]}
              >
                <Bookmark size={13} color={t.brand} weight={2.2} />
                <Text style={styles.replyText} numberOfLines={1}>
                  {snippet.name}
                </Text>
              </Pressable>
            ))}
            {REPLIES.map((text) => (
              <Pressable
                key={text}
                accessibilityRole="button"
                accessibilityLabel={`Send ${text}`}
                onPress={() => void send(text)}
                style={({ pressed }) => [styles.reply, pressed && { backgroundColor: t.selection }]}
              >
                <Text style={styles.replyText}>{text}</Text>
              </Pressable>
            ))}
          </ScrollView>
        ) : null}
        <View style={[styles.composer, { paddingBottom: rest }]}>
          <TextInput
            ref={input}
            value={message}
            onChangeText={(text) => {
              if (Date.now() - clearedAt.current < SETTLE_MS) {
                input.current?.clear();
                return;
              }
              setMessage(text);
            }}
            placeholder={`Message ${agent ? agentLabel(agent) : 'the agent'}`}
            placeholderTextColor={t.faint}
            multiline
            style={styles.input}
          />
          {working && !message.trim() ? (
            // Working, with nothing to send: the button stops the turn.
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Stop"
              disabled={stopping}
              onPress={stop}
              style={({ pressed }) => [styles.send, styles.sendStop, (pressed || stopping) && { opacity: 0.6 }]}
            >
              <Stop size={18} color={t.text} />
            </Pressable>
          ) : (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Send"
              disabled={!message.trim()}
              onPress={() => void send()}
              style={[styles.send, message.trim() ? styles.sendReady : null]}
            >
              <Send size={18} color={message.trim() ? t.onBrand : t.faint} weight={2.3} />
            </Pressable>
          )}
        </View>
        </KeyboardStickyView>
      </View>
    </Screen>
  );
}

/** The conversation's foot, faded into the desk while there is more below
 * it — Android's fading edge — so a message scrolled up past the composer
 * thins out rather than stopping at a hard line. Hangs above the composer,
 * riding up with it and the keyboard; drawn under the jump button. */
function FadingEdge({ shown }: { shown: boolean }) {
  const strength = useSharedValue(shown ? 1 : 0);
  useEffect(() => {
    strength.set(withTiming(shown ? 1 : 0, { duration: EDGE_MS }));
  }, [shown, strength]);
  const fading = useAnimatedStyle(() => ({ opacity: strength.get() }));
  return (
    <Animated.View pointerEvents="none" style={[styles.edge, fading]}>
      <Svg width="100%" height={EDGE_H}>
        <Defs>
          <LinearGradient id="conversation-foot" x1="0" y1="0" x2="0" y2="1">
            <GradientStop offset="0" stopColor={t.backdrop} stopOpacity={0} />
            <GradientStop offset="1" stopColor={t.backdrop} stopOpacity={1} />
          </LinearGradient>
        </Defs>
        <Rect width="100%" height={EDGE_H} fill="url(#conversation-foot)" />
      </Svg>
    </Animated.View>
  );
}

/** Appends `more`, running an agent's reply on when it arrived in two reads. */
function join(before: Turn[], more: Turn[]): Turn[] {
  const last = before[before.length - 1];
  const [first, ...rest] = more;
  if (last && first && last.kind === Turn_Kind.ASSISTANT && first.kind === Turn_Kind.ASSISTANT) {
    return [...before.slice(0, -1), { ...last, text: `${last.text}\n\n${first.text}` }, ...rest];
  }
  return [...before, ...more];
}

/** Which card a prompt takes, when it is not a permission. */
function promptKind(tool: string): 'ask' | 'plan' | undefined {
  if (tool === 'AskUserQuestion') return 'ask';
  if (tool === 'ExitPlanMode') return 'plan';
  return undefined;
}

/** A question the agent asks with options, or a plan it wants to go ahead
 * with: answered here and handed to the agent by the desktop. When the
 * desktop cannot take the answer — an older ket, or a wait that ran out —
 * it says so and opens the terminal instead. */
function Asking({
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

  if (!question.answerable || (kind === 'ask' && question.asks.length === 0)) {
    return (
      <View style={styles.ask}>
        <Text style={styles.askTitle} numberOfLines={1}>
          {kind === 'plan' ? 'Has a plan ready' : 'Has a question'}
        </Text>
        {question.subject ? (
          <Text style={styles.askLead} numberOfLines={3}>
            {question.subject}
          </Text>
        ) : null}
        <Button kind="primary" title="Answer in the terminal" onPress={onTerminal} />
      </View>
    );
  }

  if (kind === 'plan') {
    return (
      <View style={styles.ask}>
        <Text style={styles.askTitle}>Ready to code?</Text>
        {question.subject ? (
          <Text style={styles.askLead} numberOfLines={2}>
            {question.subject}
          </Text>
        ) : null}
        {refining ? (
          <>
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
            <View style={styles.askButtons}>
              <Button
                kind="primary"
                title="Send"
                disabled={!idle || !feedback.trim()}
                onPress={() => void send({ approve: false, feedback: feedback.trim() })}
                style={styles.grow}
              />
              <Button kind="ghost" title="Cancel" disabled={!idle} onPress={() => setRefining(false)} style={styles.grow} />
            </View>
          </>
        ) : (
          <View style={styles.askButtons}>
            <Button kind="primary" title="Approve" disabled={!idle} onPress={() => void send({ approve: true })} style={styles.grow} />
            <Button title="Keep planning" disabled={!idle} onPress={() => setRefining(true)} style={styles.grow} />
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
    <View style={styles.ask}>
      <ScrollView style={styles.askScroll} contentContainerStyle={styles.askQuestions} keyboardShouldPersistTaps="handled">
        {question.asks.map((asked, i) => (
          <View key={i} style={styles.asked}>
            {asked.header ? <Text style={styles.askHeader}>{asked.header}</Text> : null}
            <Text style={styles.askTitle}>{asked.question}</Text>
            {asked.multiSelect ? <Text style={styles.askLead}>Pick any</Text> : null}
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
            <TextInput
              value={others[i]}
              onChangeText={(text) => write(i, text)}
              placeholder="Or write your own"
              placeholderTextColor={t.faint}
              editable={idle}
              style={field}
            />
          </View>
        ))}
      </ScrollView>
      <View style={styles.askButtons}>
        <Button
          kind="primary"
          title={question.asks.length > 1 ? 'Send answers' : 'Send answer'}
          disabled={!idle || !answered}
          onPress={() =>
            void send({ picks: question.asks.map((_, i) => ({ choices: picks[i], other: others[i].trim() })) })
          }
          style={styles.grow}
        />
        <Button kind="ghost" title="In the terminal" onPress={onTerminal} />
      </View>
    </View>
  );
}

/** The agent at work: its mark, a pulse, and what it is doing — "Thinking",
 * "Running cargo test" — until its next words reach the conversation. */
function Working({ agent, what }: { agent: string; what: string }) {
  const [phase, setPhase] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setPhase((n) => (n + 1) % 3), 450);
    return () => clearInterval(timer);
  }, []);
  return (
    <View style={styles.agent} accessibilityLabel={what || 'Waiting for the agent'}>
      {agent ? <AgentMark agent={agent} size={14} /> : null}
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

/** Whether a message the transcript or the agent's queue shows is one sent
 * from here: the same start, since the desktop clips long turns, or the same
 * end, since whatever the terminal's prompt already held when the phone
 * typed — a stray key at the desktop — arrives in front of it. */
function carries(said: string, sent: string): boolean {
  const flat = (text: string) => text.replace(/\s+/g, ' ').trim();
  const whole = flat(said);
  const mine = flat(sent);
  return whole.slice(0, 200) === mine.slice(0, 200) || whole.endsWith(mine);
}

/** A message of yours the agent has not taken in yet. Sending, until the
 * conversation or the agent's queue shows it; queued, while the agent works
 * and holds it; unseen, when neither has after a while and the agent is not
 * working — typed into a prompt, perhaps — and a tap clears it. Codex keeps
 * no queue in its rollout, so a message sent mid-turn stays sending until
 * the turn ends and Codex takes it in.
 *
 * The state is a mark beside the bubble — see [`StatusMark`] — never a line
 * of words under it, which came and went as the message was taken in and
 * made the chat jump. The bubble is the one the delivered message will be,
 * faded while sending, so arriving changes nothing but the mark — and held,
 * it copies, as the delivered one does. */
function Outgoing({
  text,
  status,
  onCopy,
  onDismiss,
}: {
  text: string;
  status: 'sending' | 'queued' | 'unseen';
  onCopy: (text: string) => void;
  onDismiss?: () => void;
}) {
  const said =
    status === 'queued'
      ? 'Queued, read when the agent finishes this step'
      : status === 'unseen'
        ? 'Not in the conversation, check the terminal'
        : 'Sending';
  return (
    <Pressable
      onPress={status === 'unseen' ? onDismiss : undefined}
      delayLongPress={350}
      onLongPress={() => onCopy(text)}
      accessibilityLabel={`${text}. ${said}.`}
      accessibilityHint={status === 'unseen' ? 'Tap to clear, hold to copy' : 'Hold to copy'}
      style={({ pressed }) => [styles.you, status === 'sending' && styles.youSending, pressed && styles.held]}
    >
      <StatusMark status={status} />
      <Text style={[styles.youText, status === 'sending' && styles.youSendingText]}>{text}</Text>
    </Pressable>
  );
}

/**
 * Where a message of yours stands, as a mark in the gutter to the left of its
 * bubble: a dashed ring while it is sent, an amber clock while the agent holds
 * it, a red ! when it never arrived, a faint tick once it is in.
 *
 * Centred on the bubble's first line, however many lines the message runs to,
 * and drawn outside the bubble's box: it never shifts the text, never changes
 * where a long message wraps, and changing from one mark to the next — or the
 * tick moving on to the next message — moves nothing else. Shape as well as
 * colour tells the four apart.
 */
function StatusMark({ status }: { status: 'sending' | 'queued' | 'unseen' | 'delivered' }) {
  return (
    <View pointerEvents="none" style={styles.statusMark}>
      <Svg width={14} height={14} viewBox="0 0 14 14">
        {status === 'sending' ? (
          <Circle cx={7} cy={7} r={5.25} fill="none" stroke={t.faint} strokeWidth={1.5} strokeDasharray="3 2.5" />
        ) : status === 'queued' ? (
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

/** A reason from the desktop as a sentence. */
function sentence(text: string): string {
  const s = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(s) ? s : `${s}.`;
}

function TurnView({
  turn,
  agent,
  onCopy,
  delivered,
}: {
  turn: Turn;
  agent: string;
  onCopy: (text: string) => void;
  delivered?: boolean;
}) {
  // A tool line opened to its whole text.
  const [open, setOpen] = useState(false);
  switch (turn.kind) {
    case Turn_Kind.USER:
      return (
        <Pressable
          accessibilityHint="Hold to copy"
          delayLongPress={350}
          onLongPress={() => onCopy(turn.text)}
          style={({ pressed }) => [styles.you, pressed && styles.held]}
        >
          {delivered ? <StatusMark status="delivered" /> : null}
          <Text style={styles.youText}>
            <Linked text={turn.text} />
          </Text>
        </Pressable>
      );
    case Turn_Kind.TOOL:
      return (
        <Pressable
          accessibilityHint={open ? 'Tap to shorten' : 'Tap to show all of it'}
          onPress={() => setOpen(!open)}
          delayLongPress={350}
          onLongPress={() => onCopy(turn.text)}
          style={({ pressed }) => [styles.tool, pressed && styles.held]}
        >
          <View style={styles.toolMark} />
          <Text style={styles.toolText} numberOfLines={open ? undefined : 2}>
            {turn.text}
          </Text>
        </Pressable>
      );
    default:
      return (
        <Pressable
          accessibilityHint="Hold to copy"
          delayLongPress={350}
          onLongPress={() => onCopy(turn.text)}
          style={styles.agent}
        >
          {agent ? <AgentMark agent={agent} size={14} /> : null}
          <View style={styles.agentBody}>
            <Words text={turn.text} onCopy={onCopy} />
          </View>
        </Pressable>
      );
  }
}

/** An agent's words: fenced code as a block of its own, inline code in the
 * mono face, the rest as prose — its markdown headings, bullets and emphasis
 * drawn rather than shown as marks. */
function Words({ text, onCopy }: { text: string; onCopy: (text: string) => void }) {
  const parts = text.split('```');
  return (
    <>
      {parts.map((part, index) => {
        if (index % 2 === 1) {
          const code = part.replace(/^[a-zA-Z0-9_+-]*\n/, '').replace(/\n$/, '');
          return (
            <View key={index} style={styles.codeBlock}>
              <ScrollView horizontal contentContainerStyle={styles.codeBlockInner}>
                <Text style={styles.codeText}>{code}</Text>
              </ScrollView>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Copy code"
                hitSlop={6}
                onPress={() => onCopy(code)}
                style={({ pressed }) => [styles.codeCopy, pressed && { backgroundColor: t.selection }]}
              >
                <Text style={styles.codeCopyText}>Copy</Text>
              </Pressable>
            </View>
          );
        }
        return blocks(part).map((block, at) =>
          block.kind === 'table' ? (
            <Table key={`${index}-${at}`} table={block} />
          ) : (
            <View key={`${index}-${at}`} style={styles.prose}>
              {lines(block.text).map((line, n) => (
                <Line key={n} line={line} />
              ))}
            </View>
          ),
        );
      })}
    </>
  );
}

/** One line of prose, as markdown means it. */
type ProseLine =
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'item'; depth: number; marker: string; text: string }
  | { kind: 'task'; depth: number; done: boolean; text: string }
  | { kind: 'quote'; text: string }
  | { kind: 'rule' }
  | { kind: 'para'; text: string };

/** The leading spaces before a list marker, as nesting: two to a level. */
const depthOf = (spaces: string) => Math.min(4, Math.floor(spaces.replace(/\t/g, '  ').length / 2));

/** Reads prose into headings, list items, task items, quotes, rules and
 * paragraphs. A line that carries on a list item or a quote, without a
 * marker of its own, joins it; lines in a row are one paragraph. */
function lines(text: string): ProseLine[] {
  const out: ProseLine[] = [];
  let open: ProseLine | null = null;
  for (const raw of text.split('\n')) {
    const line = raw.trimEnd();
    if (!line.trim()) {
      open = null;
      continue;
    }
    let m: RegExpMatchArray | null;
    let next: ProseLine;
    if ((m = line.match(/^\s*(#{1,6})\s+(.+)$/))) {
      next = { kind: 'heading', level: m[1].length, text: m[2].replace(/\s+#+$/, '') };
    } else if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      next = { kind: 'rule' };
    } else if ((m = line.match(/^(\s*)[-*+]\s+\[([ xX])\]\s+(.*)$/))) {
      next = { kind: 'task', depth: depthOf(m[1]), done: m[2] !== ' ', text: m[3] };
    } else if ((m = line.match(/^(\s*)[-*+]\s+(.*)$/))) {
      next = { kind: 'item', depth: depthOf(m[1]), marker: '\u2022', text: m[2] };
    } else if ((m = line.match(/^(\s*)(\d+)[.)]\s+(.*)$/))) {
      next = { kind: 'item', depth: depthOf(m[1]), marker: `${m[2]}.`, text: m[3] };
    } else if ((m = line.match(/^\s*>\s?(.*)$/))) {
      if (open?.kind === 'quote') {
        open.text += `\n${m[1]}`;
        continue;
      }
      next = { kind: 'quote', text: m[1] };
    } else if (open && (open.kind === 'item' || open.kind === 'task') && /^\s/.test(line)) {
      open.text += ` ${line.trim()}`;
      continue;
    } else if (open?.kind === 'para') {
      open.text += `\n${line}`;
      continue;
    } else {
      next = { kind: 'para', text: line };
    }
    out.push(next);
    open = next;
  }
  return out;
}

/** A heading's size, by level: the body's for the fourth and below. */
const HEADING = [
  { fontSize: 18, lineHeight: 25 },
  { fontSize: 16.5, lineHeight: 23 },
  { fontSize: 15, lineHeight: 22 },
];

/** One line of prose, drawn. A list item hangs its wrapped lines under its
 * words, not its marker. */
function Line({ line }: { line: ProseLine }) {
  switch (line.kind) {
    case 'heading':
      return (
        <Text style={[styles.agentText, styles.strong, HEADING[Math.min(line.level, 3) - 1], line.level <= 2 && styles.headingGap]}>
          <Inline text={line.text} />
        </Text>
      );
    case 'item':
      return (
        <View style={[styles.item, { paddingLeft: line.depth * 16 }]}>
          <Text style={[styles.agentText, styles.marker]}>{line.marker}</Text>
          <Text style={[styles.agentText, styles.itemText]}>
            <Inline text={line.text} />
          </Text>
        </View>
      );
    case 'task':
      return (
        <View style={[styles.item, { paddingLeft: line.depth * 16 }]}>
          <View style={[styles.box, line.done && styles.boxDone]}>
            {line.done ? <Check size={11} color={t.onAccent} weight={3} /> : null}
          </View>
          <Text style={[styles.agentText, styles.itemText, line.done && { color: t.dim }]}>
            <Inline text={line.text} />
          </Text>
        </View>
      );
    case 'quote':
      return (
        <View style={styles.quote}>
          <Text style={[styles.agentText, { color: t.dim }]}>
            <Inline text={line.text} />
          </Text>
        </View>
      );
    case 'rule':
      return <View style={styles.rule} />;
    default:
      return (
        <Text style={styles.agentText}>
          <Inline text={line.text} />
        </Text>
      );
  }
}

/** A line's own markdown: `code`, **strong**, *emphasis* and links. */
function Inline({ text }: { text: string }) {
  return (
    <>
      {text.split(/(`[^`\n]+`)/).map((piece, at) =>
        piece.startsWith('`') && piece.endsWith('`') && piece.length > 2 ? (
          <Text key={at} style={styles.inlineCode}>
            {piece.slice(1, -1)}
          </Text>
        ) : (
          <Emphasis key={at} text={piece} />
        ),
      )}
    </>
  );
}

/** A markdown table, read out of the words around it. */
type TableBlock = { kind: 'table'; head: string[]; align: ('left' | 'center' | 'right')[]; rows: string[][] };

/** A run of an agent's words outside code: prose, or a table. */
type Block = { kind: 'prose'; text: string } | TableBlock;

/** The rule under a table's header: `|---|:--:|--:|`. */
const TABLE_RULE = /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$/;

/** A table row's cells, without the pipes around them. An escaped `\|` is
 * a pipe in a cell; `<br>`, which agents put in cells, keeps it on a line. */
function cells(line: string): string[] {
  let row = line.trim().replace(/\\\|/g, '\u0000');
  if (row.startsWith('|')) row = row.slice(1);
  if (row.endsWith('|')) row = row.slice(0, -1);
  return row.split('|').map((cell) =>
    cell
      .trim()
      .replace(/\u0000/g, '|')
      .replace(/<br\s*\/?>/gi, ' '),
  );
}

/** Splits words outside code into prose and tables: a table is a header row,
 * the rule under it, and the rows that follow, each with a pipe in it. */
function blocks(text: string): Block[] {
  const lines = text.split('\n');
  const out: Block[] = [];
  let prose: string[] = [];
  const flush = () => {
    const words = prose.join('\n').trim();
    if (words) out.push({ kind: 'prose', text: words });
    prose = [];
  };
  for (let at = 0; at < lines.length; at++) {
    const line = lines[at];
    if (line.includes('|') && at + 1 < lines.length && TABLE_RULE.test(lines[at + 1])) {
      const head = cells(line);
      const align = cells(lines[at + 1]).map((rule) =>
        rule.startsWith(':') && rule.endsWith(':') ? 'center' : rule.endsWith(':') ? 'right' : 'left',
      );
      const rows: string[][] = [];
      at += 2;
      while (at < lines.length && lines[at].includes('|') && lines[at].trim()) {
        const row = cells(lines[at]);
        rows.push(head.map((_, col) => row[col] ?? ''));
        at++;
      }
      at--;
      flush();
      out.push({ kind: 'table', head, align: head.map((_, col) => align[col] ?? 'left'), rows });
      continue;
    }
    prose.push(line);
  }
  flush();
  return out;
}

/** A table, column by column so each column takes its widest cell, and
 * sideways to scroll when it is wider than the phone. Cells stay on one line,
 * which keeps the rows of every column level with each other.
 *
 * The frame and its rounded clip are on a view around the scroller, not on the
 * scroller itself: `overflow: 'hidden'` on a ScrollView replaces the `scroll`
 * its layout depends on, and the columns are then squeezed to the phone's
 * width and cut off instead of running past it. */
function Table({ table }: { table: TableBlock }) {
  return (
    <View style={styles.table}>
    <ScrollView horizontal contentContainerStyle={styles.tableInner}>
      {table.head.map((title, col) => {
        const align = { textAlign: table.align[col] };
        return (
          <View key={col} style={[styles.tableColumn, col > 0 && styles.tableColumnRule]}>
            <View style={[styles.tableCell, styles.tableHead]}>
              <Text style={[styles.tableText, styles.strong, align]} numberOfLines={1}>
                <Inline text={title} />
              </Text>
            </View>
            {table.rows.map((row, at) => (
              <View key={at} style={[styles.tableCell, styles.tableRowRule]}>
                <Text style={[styles.tableText, align]} numberOfLines={1}>
                  <Inline text={row[col]} />
                </Text>
              </View>
            ))}
          </View>
        );
      })}
    </ScrollView>
    </View>
  );
}

/** Markdown's `**strong**` and `*emphasis*`, drawn without their stars. */
function Emphasis({ text }: { text: string }) {
  return (
    <>
      {text.split(/(\*\*\*[^*\n]+\*\*\*|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|\*[^*\s][^*\n]*\*)/).map((piece, at) =>
        piece.length > 6 && piece.startsWith('***') && piece.endsWith('***') ? (
          <Text key={at} style={[styles.strong, styles.emphasis]}>
            <Linked text={piece.slice(3, -3)} />
          </Text>
        ) : piece.length > 4 && piece.startsWith('~~') && piece.endsWith('~~') ? (
          <Text key={at} style={styles.struck}>
            <Linked text={piece.slice(2, -2)} />
          </Text>
        ) : piece.length > 4 &&
          ((piece.startsWith('**') && piece.endsWith('**')) || (piece.startsWith('__') && piece.endsWith('__'))) ? (
          <Text key={at} style={styles.strong}>
            <Linked text={piece.slice(2, -2)} />
          </Text>
        ) : piece.length > 2 && piece.startsWith('*') && piece.endsWith('*') ? (
          <Text key={at} style={styles.emphasis}>
            <Linked text={piece.slice(1, -1)} />
          </Text>
        ) : (
          <Linked key={at} text={piece} />
        ),
      )}
    </>
  );
}

/** A markdown link, `[words](https://…)`, or a bare web address. */
const LINK = /\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|(https?:\/\/[^\s<>"'`]+)/g;

/** Text with its links made tappable, opened in the phone's browser. A bare
 * address leaves the punctuation that ends its sentence, and markdown's
 * emphasis around it, outside it. */
function Linked({ text }: { text: string }) {
  const out: ReactNode[] = [];
  let from = 0;
  for (const match of text.matchAll(LINK)) {
    const at = match.index ?? 0;
    let shown = match[1] ?? match[3];
    let url = match[2] ?? match[3];
    let rest = '';
    if (match[3]) {
      const trimmed = url.replace(/[.,:;!?)\]*_~]+$/, '');
      rest = url.slice(trimmed.length);
      url = shown = trimmed;
    }
    if (at > from) out.push(text.slice(from, at));
    out.push(
      <Text key={at} style={styles.link} onPress={() => void Linking.openURL(url)}>
        {shown}
      </Text>,
    );
    if (rest) out.push(rest);
    from = at + match[0].length;
  }
  if (from < text.length) out.push(text.slice(from));
  return <>{out}</>;
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  page: { paddingHorizontal: size.gutter + 4, paddingTop: 4, paddingBottom: 18, gap: 12 },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: size.caption },
  barActions: { flexDirection: 'row', gap: 4 },
  empty: { gap: 12, paddingVertical: 24, alignItems: 'flex-start' },
  // Round all over but the corner nearest the composer, which is tighter: a
  // hint of where the message came from.
  you: {
    alignSelf: 'flex-end',
    maxWidth: '86%',
    backgroundColor: t.selection,
    borderRadius: 18,
    borderBottomRightRadius: 6,
    paddingHorizontal: 12,
    paddingVertical: 9,
  },
  // Faded while sending — the fill and the words, not the mark beside them —
  // at the delivered bubble's size and corners.
  youSending: { backgroundColor: tint(t.selection, 0.7) },
  youSendingText: { opacity: 0.7 },
  // In the gutter left of the bubble, centred on the first line: the bubble's
  // top padding, then half of a 20pt line less half the 14pt mark.
  statusMark: { position: 'absolute', left: -20, top: 12 },
  youText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 20, color: t.text },
  agent: { flexDirection: 'row', gap: 10, alignItems: 'flex-start' },
  agentBody: { flex: 1, minWidth: 0, gap: 8, paddingTop: 1 },
  agentText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 21, color: t.text },
  link: { color: t.attention, textDecorationLine: 'underline' },
  strong: { fontFamily: font.proseSemibold },
  emphasis: { fontStyle: 'italic' },
  struck: { textDecorationLine: 'line-through', color: t.dim },
  held: { opacity: 0.7 },
  prose: { gap: 6 },
  headingGap: { marginTop: 4 },
  item: { flexDirection: 'row', gap: 7 },
  marker: { minWidth: 12, color: t.dim },
  itemText: { flex: 1, minWidth: 0 },
  box: {
    width: 15,
    height: 15,
    marginTop: 3,
    borderRadius: 4,
    borderWidth: 1.5,
    borderColor: t.dim,
    alignItems: 'center',
    justifyContent: 'center',
  },
  boxDone: { backgroundColor: t.accent, borderColor: t.accent },
  quote: { borderLeftWidth: 3, borderLeftColor: t.border, paddingLeft: 10 },
  rule: { height: 1, backgroundColor: t.rule, marginVertical: 6 },
  codeCopy: {
    position: 'absolute',
    top: 6,
    right: 6,
    height: 24,
    paddingHorizontal: 8,
    borderRadius: size.radiusSm,
    backgroundColor: t.elevated,
    justifyContent: 'center',
  },
  codeCopyText: { fontFamily: font.proseMedium, fontSize: 11.5, color: t.dim },
  inlineCode: { fontFamily: font.code, fontSize: size.label, color: t.text, backgroundColor: t.sunken },
  codeBlock: { backgroundColor: t.sunken, borderRadius: size.radius },
  table: { borderWidth: 1, borderColor: t.border, borderRadius: size.radius, overflow: 'hidden' },
  tableInner: { flexDirection: 'row' },
  tableColumn: { minWidth: 56 },
  tableColumnRule: { borderLeftWidth: 1, borderLeftColor: t.rule },
  tableCell: { height: 34, justifyContent: 'center', paddingHorizontal: 10 },
  tableHead: { backgroundColor: t.sunken },
  tableRowRule: { borderTopWidth: 1, borderTopColor: t.rule },
  tableText: { fontFamily: font.prose, fontSize: size.label, lineHeight: 18, color: t.text },
  codeBlockInner: { padding: 10 },
  codeText: { fontFamily: font.code, fontSize: 12.5, lineHeight: 18, color: t.text },
  working: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: 5, paddingTop: 1 },
  workingDot: { width: 5, height: 5, borderRadius: 3, backgroundColor: t.faint },
  workingText: { flex: 1, minWidth: 0, marginLeft: 4, fontFamily: font.chrome, fontSize: size.detail, color: t.dim },
  tool: { flexDirection: 'row', alignItems: 'flex-start', gap: 8, paddingLeft: 24 },
  toolMark: { width: 5, height: 5, borderRadius: 3, marginTop: 6, backgroundColor: t.faint },
  toolText: { flex: 1, minWidth: 0, fontFamily: font.chrome, fontSize: size.detail, color: t.dim },
  ask: {
    marginHorizontal: size.gutter,
    marginBottom: 8,
    padding: 12,
    gap: 8,
    borderRadius: size.radiusCard,
    backgroundColor: tint(t.attention, 0.1),
    borderWidth: 1,
    borderColor: tint(t.attention, 0.35),
  },
  askTitle: { fontFamily: font.proseSemibold, fontSize: size.body, color: t.text },
  askSubject: { fontFamily: font.code, fontSize: 12.5, lineHeight: 18, color: t.text },
  askLead: { fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  askHeader: { fontFamily: font.chrome, fontSize: size.caption, color: t.attention },
  askScroll: { flexGrow: 0, maxHeight: 380 },
  askQuestions: { gap: 16 },
  asked: { gap: 6 },
  askInput: { height: undefined, minHeight: size.control, maxHeight: 120, paddingTop: 11, paddingBottom: 11, lineHeight: 20 },
  askButtons: { flexDirection: 'row', gap: 8 },
  grow: { flex: 1 },
  note: { paddingHorizontal: size.gutter + 4, paddingBottom: 6 },
  replies: { flexGrow: 0 },
  repliesInner: { paddingHorizontal: size.gutter, paddingBottom: 8, gap: 6 },
  // A chip: a reply, a snippet, or `/`. Gone once there is a draft.
  reply: {
    height: 30,
    maxWidth: 220,
    paddingHorizontal: 11,
    borderRadius: 15,
    borderWidth: 1,
    borderColor: t.rule,
    backgroundColor: t.card,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  replyText: { fontFamily: font.proseMedium, fontSize: size.label, color: t.text },
  slash: { width: 30, paddingHorizontal: 0, justifyContent: 'center' },
  slashText: { fontFamily: font.chromeMedium, fontSize: 14, color: t.dim },
  snippet: { paddingLeft: 9 },
  // The field and Send beside it: nothing else, since everything else is a chip.
  composer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
    paddingHorizontal: size.gutter,
  },
  input: {
    flex: 1,
    minHeight: size.control,
    maxHeight: 140,
    borderRadius: size.control / 2,
    backgroundColor: t.sunken,
    paddingHorizontal: 16,
    paddingTop: 11,
    paddingBottom: 11,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
    lineHeight: 21,
    textAlignVertical: 'top',
  },
  send: {
    width: size.control,
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.selection,
    alignItems: 'center',
    justifyContent: 'center',
  },
  // Ready to send: ket's own green, the one thing on the bar that is.
  sendReady: { backgroundColor: t.brand },
  // Working: the same place stops it.
  sendStop: { backgroundColor: t.selection },
  edge: { position: 'absolute', top: -EDGE_H, left: 0, right: 0, height: EDGE_H },
  jumpDock: { position: 'absolute', top: -54, right: size.gutter + 4 },
  jump: {
    width: 40,
    height: 40,
    borderRadius: 20,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: '#000',
    shadowOpacity: 0.4,
    shadowRadius: 10,
    shadowOffset: { width: 0, height: 4 },
  },
  jumpDot: {
    position: 'absolute',
    top: 2,
    right: 2,
    width: 9,
    height: 9,
    borderRadius: 5,
    backgroundColor: t.brand,
    borderWidth: 1.5,
    borderColor: t.elevated,
  },
});
