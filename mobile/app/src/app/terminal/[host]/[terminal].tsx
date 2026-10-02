'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// A terminal on the desktop, drawn on the phone.
//
// View-only until the phone types: focusing the message line, sending, or a
// key from the key row takes control, with no button to press first. A line
// at the bottom sends a whole message; the keys a phone keyboard lacks sit
// behind a button beside it. The desktop can take control back with a
// keystroke, and the phone says so. Interrupting needs no control: stopping an
// agent is always the phone's to do.

import { router, useLocalSearchParams } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { CommandMenu } from '../../../components/Commands';
import { TerminalView, type TerminalHandle } from '../../../components/TerminalView';
import { AgentMark, Keyboard, Send, TerminalGlyph } from '../../../components/icons';
import { Bar, Card, Dot, Prose, Readout, Screen, Segmented } from '../../../components/ui';
import { agentTerminal, everyWorktree, railColor, stateOf } from '../../../lib/activity';
import { type Command, commandsFor, matching } from '../../../lib/commands';
import { useHost } from '../../../lib/hosts';
import { getFit } from '../../../lib/store';
import { font, size, t, tint } from '../../../lib/theme';

/** The pause between a message and its Enter — see `send`. */
const ENTER_AFTER_MS = 80;

const KEYS: { label: string; send: string; name?: string }[] = [
  { label: 'esc', send: '\x1b' },
  { label: 'tab', send: '\t' },
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
  // Settings › Fit terminal to phone: while the phone has control, the
  // host's terminal takes the phone's size, and the desktop's comes back
  // when control ends.
  const [fit, setFitSetting] = useState(false);
  useEffect(() => {
    void getFit().then(setFitSetting);
  }, []);
  const [ctrl, setCtrl] = useState(false);
  const [note, setNote] = useState('');
  const [message, setMessage] = useState('');

  const located = everyWorktree(conn?.snapshot ?? null)
    .map((l) => ({ ...l, terminal: l.worktree.terminals.find((x) => x.id === terminalId) }))
    .find((l) => l.terminal);
  const summary = located?.terminal;
  const isAgent = located ? agentTerminal(located.worktree)?.id === terminalId : false;
  const stateColor = located && isAgent ? railColor(located.worktree.activity) : undefined;

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
  useEffect(() => view.current?.setFit(fit && ready), [fit, ready, online]);

  const fitTo = (cols: number, rows: number) => {
    if (conn && fit) void conn.resize(terminalId, cols, rows).catch(() => {});
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
    held.current = ok;
    setControl(ok);
    setNote(ok ? '' : 'Another device is in control of this terminal.');
    return ok;
  };

  /** Types into the terminal, taking control first if need be; `false` when
   * it did not arrive, with why. */
  const type = async (data: string): Promise<boolean> => {
    if (!conn || !(await ensureControl())) return false;
    let text = data;
    if (ctrl && text.length === 1) {
      text = String.fromCharCode(text.toUpperCase().charCodeAt(0) & 0x1f);
      setCtrl(false);
    }
    try {
      if (await conn.type(terminalId, text)) return true;
      held.current = false;
      setControl(false);
      setNote('Your desktop took control back.');
    } catch (e) {
      // Not the desktop taking over: the message never reached it.
      setNote(`Not sent: ${e instanceof Error ? e.message : String(e)}`);
    }
    return false;
  };

  const release = () => {
    conn?.releaseControl(terminalId);
    held.current = false;
    setControl(false);
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
    const sent = type(text).then((ok) => {
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

  const agent = summary?.agent ?? '';

  // The `/` menu, while the draft is a slash and the start of a name.
  const suggested = matching(commandsFor(agent), message);
  const pick = (command: Command) => {
    if (command.args) setMessage(`${command.name} `);
    else void send(command.name);
  };
  const name = summary?.title || agent || 'Terminal';
  const tabs = located ? located.worktree.terminals.filter((x) => !x.closed || x.id === terminalId) : [];

  return (
    <Screen>
      <KeyboardAvoidingView style={styles.fill} behavior="padding">
        <Bar
          back
          detail={located?.worktree.name}
          lead={
            <View style={styles.titleLine}>
              {agent ? <AgentMark agent={agent} size={14} /> : <TerminalGlyph size={14} />}
              <Text style={styles.title} numberOfLines={1}>
                {name}
              </Text>
            </View>
          }
          right={
            <View style={styles.barRight}>
            {control ? (
              <Pressable accessibilityRole="button" accessibilityLabel="Let go of control" onPress={release}>
                <Readout style={styles.lease}>
                  <Dot size={6} color={t.running} />
                  <Text style={styles.leaseText}>you have control</Text>
                </Readout>
              </Pressable>
            ) : stateColor && located ? (
              <Readout style={styles.lease}>
                <Dot size={6} color={stateColor} />
                <Text style={[styles.leaseText, { color: t.dim }]} numberOfLines={1}>
                  {stateOf(located.worktree.activity)?.word}
                </Text>
              </Readout>
            ) : null}
            </View>
          }
        />

        <View style={styles.body}>
          <Card style={styles.terminalCard}>
            {tabs.length > 1 ? (
              <Segmented
                segments={tabs.map((x) => ({
                  key: x.id.toString(),
                  label: x.title || x.agent || 'shell',
                  icon: x.agent ? <AgentMark agent={x.agent} size={12} /> : <TerminalGlyph size={12} color={t.dim} />,
                }))}
                value={terminal}
                onChange={(key) => router.replace(`/terminal/${hostId}/${key}`)}
                style={styles.tabs}
              />
            ) : null}
            <View style={styles.fill}>
              <TerminalView ref={view} onReady={() => setReady(true)} onInput={(data) => void type(data)} onSize={fitTo} />
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
            {suggested.length > 0 ? <CommandMenu commands={suggested} onPick={pick} /> : null}
            {keysOpen ? (
              <View style={styles.keys}>
                <Key label="ctrl" active={ctrl} onPress={() => setCtrl(!ctrl)} />
                {KEYS.map((key) => (
                  <Key key={key.label} label={key.label} name={key.name} onPress={() => void type(key.send)} />
                ))}
                <Key label="^C" name="Interrupt" danger onPress={() => conn?.interrupt(terminalId)} />
              </View>
            ) : null}
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
              <TextInput
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
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Send"
                disabled={!message}
                onPress={() => void send()}
                style={[styles.send, message ? styles.sendReady : null]}
              >
                <Send size={18} color={message ? t.onBrand : t.faint} weight={2.3} />
              </Pressable>
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
  lease: { height: 30 },
  leaseText: { fontFamily: font.chrome, fontSize: 12, color: t.text },
  body: { flex: 1, paddingHorizontal: size.gutter, gap: size.gutter },
  terminalCard: { flex: 1, padding: 8, gap: 8 },
  tabs: { flexGrow: 0 },
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
  barRight: { flexDirection: 'row', alignItems: 'center', gap: 6 },
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
