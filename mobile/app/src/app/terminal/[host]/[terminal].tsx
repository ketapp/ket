'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// A terminal on the desktop, drawn on the phone.
//
// View-only until the phone types: focusing the message line, sending, or a
// key from the key row takes control, with no button to press first, and
// leaving the screen lets it go. A line at the bottom sends a whole message;
// switched to live keys, it sends each key as it is pressed instead, for a
// program that answers a lone keystroke — a digit for Claude's offers, y or
// n, a pager. The keys a phone keyboard lacks sit behind a button beside it. The desktop
// can take control back with a keystroke, and the phone says so. Interrupting
// needs no control: stopping an agent is always the phone's to do.

import { router, useLocalSearchParams } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import {
  Keyboard as SystemKeyboard,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  type TextInputInstance,
  View,
} from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { CommandMenu } from '../../../components/Commands';
import { TerminalView, type TerminalHandle } from '../../../components/TerminalView';
import { AgentMark, Keyboard, Send, TerminalGlyph } from '../../../components/icons';
import { Bar, Card, Dot, Marked, Prose, Screen, Segmented, TabStrip } from '../../../components/ui';
import { agentTerminal, everyWorktree, stateOf, terminalName } from '../../../lib/activity';
import { type Command, commandsFor, matching } from '../../../lib/commands';
import { useHost } from '../../../lib/hosts';
import { font, size, t, tint } from '../../../lib/theme';

/** The pause between a message and its Enter — see `send`. */
const ENTER_AFTER_MS = 80;

/** What a terminal's Backspace sends. */
const BACKSPACE = '\x7f';

/** How far apart a key and the change it made are told of — see `unpaired`. */
const PAIRED_MS = 100;

/** How the line at the bottom types: a whole message and Enter, or each key
 * as it is pressed. */
type Typing = 'message' | 'live';

const KEYS: { label: string; send: string; name?: string }[] = [
  { label: 'esc', send: '\x1b' },
  { label: 'tab', send: '\t' },
  { label: '⇧⇥', send: '\x1b[Z', name: 'Shift Tab' },
  { label: '←', send: '\x1b[D', name: 'Left' },
  { label: '↓', send: '\x1b[B', name: 'Down' },
  { label: '↑', send: '\x1b[A', name: 'Up' },
  { label: '→', send: '\x1b[C', name: 'Right' },
  { label: '⏎', send: '\r', name: 'Enter' },
];

export default function TerminalScreen() {
  const { host: hostId, terminal } = useLocalSearchParams<{ host: string; terminal: string }>();
  const terminalId = BigInt(terminal);
  const insets = useSafeAreaInsets();
  const view = useRef<TerminalHandle>(null);
  const conn = useHost(hostId);
  const [ready, setReady] = useState(false);
  // Which terminal the desktop has sent a picture of, and why not when it
  // refused: an empty terminal said nothing about either. By terminal, since
  // a tab switches terminals in place and the new one is not drawn yet.
  const [drawnFor, setDrawnFor] = useState<bigint | null>(null);
  const [refusal, setRefusal] = useState<{ terminal: bigint; why: string } | null>(null);
  const drawn = drawnFor === terminalId;
  const refused = refusal?.terminal === terminalId ? refusal.why : '';
  const [control, setControl] = useState(false);
  // What `control` will be once React catches up: a keystroke that takes
  // control and types in one go must not wait for a render in between.
  const held = useRef(false);
  const [keysOpen, setKeysOpen] = useState(false);
  // While the phone shows a terminal it takes the phone's size, so text
  // wraps to the screen at a size you can read; the desktop gets its own
  // size back when the phone leaves.
  const [ctrl, setCtrl] = useState(false);
  // What `ctrl` is this instant: two keys pressed before a render must not
  // both take the latch.
  const ctrlOn = useRef(false);
  const [note, setNote] = useState('');
  const [message, setMessage] = useState('');
  // Kept across tabs: a choice about how to type, not about one terminal.
  const [typing, setTyping] = useState<Typing>('message');
  const live = typing === 'live';
  // Live keys: what the field holds, as the terminal has been sent it. The
  // field is left to itself rather than held empty — emptying it after each
  // key races the next one and types it twice — and each change is sent as
  // Backspaces and new keys from where it parts from this.
  const liveField = useRef<TextInputInstance>(null);
  const liveText = useRef('');
  // The keys in the order pressed: the first may wait on taking control,
  // and the ones after it must not overtake it.
  const liveQueue = useRef<Promise<unknown>>(Promise.resolve());
  // When the last key went: an Enter right behind a key reads as part of a
  // paste, as in `send`, which keys held back until control arrive would be.
  const lastKeyAt = useRef(0);
  // The terminal on screen now, so a grant of control still on its way when
  // a tab switches terminals is given back rather than kept.
  const shown = useRef(terminalId);
  // Bumped to drop every key still queued: on a tab switching terminals, and
  // when a key does not arrive — the desktop took control back, or the
  // connection failed — so what was typed after it is not replayed on top.
  const keysFor = useRef(0);
  // A Backspace that deletes is told of twice, as a key and as a change, in
  // either order: iOS and some Android keyboards the key first, others the
  // change. Whichever comes first waits here, briefly, for the other, and the
  // pair counts once, as the change; a key with no change — Backspace in an
  // empty field, a held Backspace's repeats there — goes on its own.
  const unpaired = useRef<{ by: 'key' | 'change'; at: number } | null>(null);
  // Whether the keyboard was up when switching, so the other field takes it.
  const [focusOnSwitch, setFocusOnSwitch] = useState(false);
  // A tab switches terminals in place, on this same screen, so what belonged
  // to the last one does not carry over: its control (let go by the effect
  // below), a note about it, a half-typed message, a held ctrl. Reset while
  // rendering, so the effects never see the new terminal with the old state.
  const [shownFor, setShownFor] = useState(terminalId);
  if (shownFor !== terminalId) {
    setShownFor(terminalId);
    setControl(false);
    setNote('');
    setMessage('');
    setCtrl(false);
    setFocusOnSwitch(false);
  }
  useEffect(() => {
    shown.current = terminalId;
    held.current = false;
    ctrlOn.current = false;
    liveText.current = '';
    keysFor.current++;
    liveQueue.current = Promise.resolve();
  }, [terminalId]);

  /** Drops the keys still queued — see `keysFor`. */
  const dropKeys = () => {
    keysFor.current++;
    liveQueue.current = Promise.resolve();
  };

  const located = everyWorktree(conn?.snapshot ?? null)
    .map((l) => ({ ...l, terminal: l.worktree.terminals.find((x) => x.id === terminalId) }))
    .find((l) => l.terminal);
  const summary = located?.terminal;
  // The worktree's state belongs to its agent's terminal, and rides on that
  // terminal's mark — in the tabs, or beside the title when there is one.
  const agentId = located ? agentTerminal(located.worktree)?.id : undefined;
  const stateFor = (id: bigint) => (located && id === agentId ? stateOf(located.worktree.activity) : undefined);

  // Subscribe once the page can draw, and again after every reconnect.
  useEffect(() => {
    if (!conn || !ready) return;
    let wasConnected = conn.status === 'connected';
    const onRefused = (why: string) => setRefusal({ terminal: terminalId, why });
    if (wasConnected) conn.subscribe(terminalId, onRefused);
    const offChange = conn.onChange(() => {
      const connected = conn.status === 'connected';
      if (connected && !wasConnected) {
        conn.subscribe(terminalId, onRefused);
        held.current = false;
        setControl(false);
      }
      wasConnected = connected;
    });
    const offMessage = conn.onMessage((m) => {
      const p = m.payload;
      if (p.case === 'checkpoint' && p.value.terminalId === terminalId) {
        view.current?.reset({ cols: p.value.cols, rows: p.value.rows, scrollback: p.value.scrollback, ansi: p.value.ansi });
        setDrawnFor(terminalId);
        setRefusal(null);
      } else if (p.case === 'chunk' && p.value.terminalId === terminalId) {
        view.current?.write(p.value.bytes);
      } else if (p.case === 'closed' && p.value.terminalId === terminalId) {
        setNote('The program has exited.');
      }
    });
    return () => {
      offChange();
      offMessage();
      conn.unsubscribe(terminalId);
    };
  }, [conn, ready, terminalId]);

  useEffect(() => view.current?.setInteractive(control), [control]);
  // Fitted with or without control: the phone watches at its own size too.
  // Said again on every reconnect — the desktop gives its size back when a
  // phone's session ends, so a new session has to ask afresh.
  const online = conn?.status === 'connected';
  useEffect(() => view.current?.setFit(ready), [ready, online]);

  const fitTo = (cols: number, rows: number) => {
    if (conn) void conn.resize(terminalId, cols, rows).catch(() => {});
  };

  // Letting go when leaving the screen.
  useEffect(
    () => () => {
      if (conn && control) conn.releaseControl(terminalId);
    },
    [conn, control, terminalId],
  );

  /** Takes control if the phone does not have it; `false` when another
   * device does. */
  const ensureControl = async (): Promise<boolean> => {
    if (held.current) return true;
    if (!conn) return false;
    const ok = await conn.takeControl(terminalId).catch(() => false);
    // Granted for a terminal a tab has since switched away from.
    if (shown.current !== terminalId) {
      if (ok) conn.releaseControl(terminalId);
      return false;
    }
    held.current = ok;
    setControl(ok);
    setNote(ok ? '' : 'Another device is in control of this terminal.');
    return ok;
  };

  /** A lone key as ctrl makes it, while ctrl is held, letting go of ctrl. */
  const latch = (text: string): string => {
    if (!ctrlOn.current || text.length !== 1) return text;
    ctrlOn.current = false;
    setCtrl(false);
    return String.fromCharCode(text.toUpperCase().charCodeAt(0) & 0x1f);
  };

  const toggleCtrl = () => {
    ctrlOn.current = !ctrlOn.current;
    setCtrl(ctrlOn.current);
  };

  /** Types into the terminal, taking control first if need be; `false` when
   * it did not arrive, with why. */
  const type = async (data: string): Promise<boolean> => {
    if (!conn || !(await ensureControl())) return false;
    try {
      if (await conn.type(terminalId, data)) return true;
      // Refused for a terminal a tab has since switched away from: nothing
      // here to drop or let go.
      if (shown.current !== terminalId) return false;
      dropKeys();
      held.current = false;
      setControl(false);
      setNote('Your desktop took control back.');
    } catch (e) {
      // Not the desktop taking over: the message never reached it.
      if (shown.current !== terminalId) return false;
      dropKeys();
      setNote(`Not sent: ${e instanceof Error ? e.message : String(e)}`);
    }
    return false;
  };

  // The text, then Enter as a keystroke of its own. Sent together, an agent's
  // prompt reads the chunk as a paste and the Enter as a new line in it, so
  // the message sits in the box on the desktop unsent.
  //
  // The Enter goes out on a timer from the text leaving, not from the desktop
  // acknowledging it: waiting for the acknowledgement added a round trip to
  // every message. Control is settled first, so the text's request cannot be
  // held up behind taking it while the Enter overtakes.
  const send = async (said?: string) => {
    const text = said ?? message;
    if (!text) return;
    setMessage('');
    if (!(await ensureControl())) {
      setMessage(text);
      return;
    }
    let failed = false;
    const sent = type(latch(text)).then((ok) => {
      failed = !ok;
      return ok;
    });
    await new Promise((resolve) => setTimeout(resolve, ENTER_AFTER_MS));
    if (failed) {
      setMessage(text);
      return;
    }
    const entered = type('\r');
    if (!(await sent)) setMessage(text);
    await entered;
  };

  /** Types `keys` after any still waiting on control: a live key, the key
   * row, the terminal's own keyboard. Ctrl is taken now, by the key pressed
   * while it was held, not by whichever key reaches the terminal first. */
  const press = (keys: string) => {
    const text = latch(keys);
    const batch = keysFor.current;
    liveQueue.current = liveQueue.current.then(async () => {
      const early = lastKeyAt.current + ENTER_AFTER_MS - Date.now();
      if (text === '\r' && early > 0) await new Promise((resolve) => setTimeout(resolve, early));
      // Checked last thing before control, which a dropped key must not
      // take back from the desktop.
      if (batch !== keysFor.current) return;
      if (!(await ensureControl())) {
        if (batch === keysFor.current) dropKeys();
        return;
      }
      if (batch !== keysFor.current) return;
      lastKeyAt.current = Date.now();
      void type(text);
    });
  };

  /** Whether a Backspace's other half is waiting to be paired — see
   * `unpaired`. */
  const waiting = (by: 'key' | 'change') =>
    unpaired.current?.by === by && Date.now() - unpaired.current.at < PAIRED_MS;

  // By character, not UTF-16 unit: an emoji is one Backspace, and a change
  // inside one never sends half of it.
  const changeLive = (text: string) => {
    const was = Array.from(liveText.current);
    const now = Array.from(text);
    let same = 0;
    while (same < was.length && same < now.length && was[same] === now[same]) same++;
    liveText.current = text;
    if (now.length < was.length && same === now.length) {
      unpaired.current = waiting('key') ? null : { by: 'change', at: Date.now() };
    }
    const keys = BACKSPACE.repeat(was.length - same) + now.slice(same).join('');
    if (keys) press(keys);
  };

  // Enter starts the field afresh: what came before it is the terminal's now.
  const enterLive = () => {
    liveText.current = '';
    liveField.current?.clear();
    press('\r');
  };

  const switchTyping = (to: Typing) => {
    if (to === typing) return;
    setFocusOnSwitch(SystemKeyboard.isVisible());
    liveText.current = '';
    setTyping(to);
  };

  const agent = summary?.agent ?? '';

  // The `/` menu, while the draft is a slash and the start of a name.
  const suggested = matching(commandsFor(agent), message);
  const pick = (command: Command) => {
    if (command.args) setMessage(`${command.name} `);
    else void send(command.name);
  };
  const name = summary ? terminalName(summary) : 'Terminal';
  const state = stateFor(terminalId);
  // With more than one, the tabs stand in the bar in the title's place: the
  // chosen tab already names the terminal shown.
  const tabs = located ? located.worktree.terminals.filter((x) => !x.closed || x.id === terminalId) : [];

  return (
    <Screen full>
      <KeyboardAvoidingView style={styles.fill} behavior="padding">
        <Bar
          back
          detail={located?.worktree.name}
          tabs={
            tabs.length > 1 ? (
              <TabStrip
                label={`Terminals in ${located?.worktree.name ?? 'this worktree'}`}
                tabs={tabs.map((x) => ({
                  key: x.id.toString(),
                  label: terminalName(x),
                  icon: x.agent ? <AgentMark agent={x.agent} size={12} /> : <TerminalGlyph size={12} color={t.dim} />,
                  state: stateFor(x.id),
                }))}
                value={terminal}
                onChange={(key) => router.setParams({ terminal: key })}
              />
            ) : undefined
          }
          lead={
            tabs.length > 1 ? undefined : (
              <View style={styles.titleLine} accessible accessibilityLabel={state ? `${name}, ${state.word}` : name}>
                <Marked dot={state?.color} ring={t.backdrop}>
                  {agent ? <AgentMark agent={agent} size={14} /> : <TerminalGlyph size={14} />}
                </Marked>
                <Text style={styles.title} numberOfLines={1}>
                  {name}
                </Text>
              </View>
            )
          }
        />

        <View style={styles.body}>
          <Card style={styles.terminalCard}>
            <View style={styles.fill}>
              <TerminalView ref={view} onReady={() => setReady(true)} onInput={press} onSize={fitTo} />
              {drawn ? null : (
                <View pointerEvents="none" style={styles.waiting} accessibilityLiveRegion="polite">
                  <Prose>
                    {conn?.status !== 'connected'
                      ? 'Waiting for the desktop…'
                      : refused
                        ? `The desktop isn’t sending this terminal: ${refused}. Still trying.`
                        : 'Opening the terminal…'}
                  </Prose>
                </View>
              )}
            </View>
          </Card>

          <Card style={[styles.panel, { marginBottom: Math.max(insets.bottom, size.gutter) }]}>
            {note ? <Prose style={styles.small}>{note}</Prose> : null}
            {!live && suggested.length > 0 ? <CommandMenu commands={suggested} onPick={pick} /> : null}
            {keysOpen ? (
              <View style={styles.keys}>
                <Key label="ctrl" active={ctrl} onPress={toggleCtrl} />
                {KEYS.map((key) => (
                  <Key key={key.label} label={key.label} name={key.name} onPress={() => press(key.send)} />
                ))}
                <Key label="^C" name="Interrupt" danger onPress={() => conn?.interrupt(terminalId)} />
              </View>
            ) : null}
            <Segmented
              segments={[
                { key: 'message', label: 'Message' },
                { key: 'live', label: 'Live keys', icon: live ? <Dot color={t.brand} /> : undefined },
              ]}
              value={typing}
              onChange={switchTyping}
            />
            <View style={styles.composer}>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={keysOpen ? 'Hide keys' : 'Show keys'}
                accessibilityState={{ expanded: keysOpen }}
                onPress={() => setKeysOpen(!keysOpen)}
                style={[styles.menu, keysOpen && { backgroundColor: t.selection }]}
              >
                <Keyboard size={18} color={keysOpen ? t.text : t.dim} />
              </Pressable>
              {live ? (
                <TextInput
                  // A new field for each terminal: what the last one holds
                  // was typed into that terminal, not this one.
                  key={`live-${terminal}`}
                  ref={liveField}
                  autoFocus={focusOnSwitch}
                  defaultValue=""
                  onChangeText={changeLive}
                  // Backspace on an empty field changes nothing, so it is
                  // only heard as a key; one that changes the field is left
                  // to `changeLive` — see `unpaired`.
                  onKeyPress={(e) => {
                    if (e.nativeEvent.key !== 'Backspace') return;
                    if (waiting('change')) unpaired.current = null;
                    else if (liveText.current !== '') unpaired.current = { by: 'key', at: Date.now() };
                    else press(BACKSPACE);
                  }}
                  onFocus={() => void ensureControl()}
                  onSubmitEditing={enterLive}
                  // The field stays up for the next key: `submitBehavior` on
                  // the phone, `blurOnSubmit` in the browser, which knows
                  // only that.
                  submitBehavior="submit"
                  blurOnSubmit={false}
                  accessibilityLabel="Live keys"
                  placeholder="Each key goes straight in"
                  placeholderTextColor={t.faint}
                  autoCapitalize="none"
                  autoCorrect={false}
                  spellCheck={false}
                  smartInsertDelete={false}
                  returnKeyType="default"
                  style={[styles.input, styles.liveInput]}
                />
              ) : (
                <TextInput
                  key="message"
                  autoFocus={focusOnSwitch}
                  value={message}
                  onChangeText={setMessage}
                  onFocus={() => void ensureControl()}
                  onSubmitEditing={() => void send()}
                  placeholder={`Message ${agent || 'the terminal'}…`}
                  placeholderTextColor={t.faint}
                  autoCapitalize="sentences"
                  autoCorrect
                  spellCheck
                  returnKeyType="send"
                  style={styles.input}
                />
              )}
              {live ? (
                <Pressable accessibilityRole="button" accessibilityLabel="Enter" onPress={enterLive} style={styles.send}>
                  <Text style={styles.enterText}>⏎</Text>
                </Pressable>
              ) : (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel="Send"
                  disabled={!message}
                  onPress={() => void send()}
                  style={[styles.send, message ? styles.sendReady : null]}
                >
                  <Send size={18} color={message ? t.onBrand : t.faint} weight={2.3} />
                </Pressable>
              )}
            </View>
          </Card>
        </View>
      </KeyboardAvoidingView>
    </Screen>
  );
}

function Key({
  label,
  onPress,
  active,
  danger,
  name,
}: {
  label: string;
  onPress: () => void;
  active?: boolean;
  danger?: boolean;
  name?: string;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={name ?? label}
      onPress={onPress}
      style={({ pressed }) => [
        styles.key,
        danger && { backgroundColor: tint(t.failed, 0.1) },
        (pressed || active) && { backgroundColor: t.selection },
      ]}
    >
      <Text style={[styles.keyText, active && { color: t.text }, danger && { color: t.failed }]}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  waiting: { position: 'absolute', top: 12, left: 12, right: 12 },
  titleLine: { flexDirection: 'row', alignItems: 'center', gap: 7 },
  title: { fontFamily: font.proseSemibold, fontSize: size.value, color: t.text },
  body: { flex: 1, paddingHorizontal: size.gutter, gap: size.gutter },
  terminalCard: { flex: 1, padding: 8 },
  panel: { padding: 8, gap: 8 },
  small: { fontSize: size.detail, lineHeight: 17, paddingHorizontal: 4 },
  keys: { flexDirection: 'row', gap: 5 },
  key: {
    flex: 1,
    height: 38,
    borderRadius: 6,
    backgroundColor: t.elevated,
    alignItems: 'center',
    justifyContent: 'center',
  },
  keyText: { fontFamily: font.chrome, fontSize: 12.5, color: t.dim },
  composer: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  // The chat's composer, field and Send alike, so the two screens type the
  // same way. One line here, though: Return sends to a terminal.
  input: {
    flex: 1,
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.sunken,
    paddingHorizontal: 16,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
  },
  send: {
    width: size.control,
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.selection,
    alignItems: 'center',
    justifyContent: 'center',
  },
  // Something to send: ket's own green, as in the chat.
  sendReady: { backgroundColor: t.brand },
  // Live keys: ringed in the brand's green, so the field never passes for
  // the message line, and in the face of what a terminal draws.
  liveInput: { fontFamily: font.chrome, fontSize: 14, borderWidth: 1, borderColor: tint(t.brand, 0.35) },
  enterText: { fontFamily: font.chrome, fontSize: 16, color: t.text },
  // Round, beside the round field and Send.
  menu: {
    width: size.control,
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.sunken,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
