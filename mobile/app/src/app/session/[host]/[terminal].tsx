'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// An agent's session as a conversation: what you said, what it said back,
// and each tool it used as one line, a run of them shut under their count —
// read from its transcript on the desktop, not drawn from a terminal screen.
// A message line under it types into the agent's terminal, with photos and
// files from + and words from the mic; when the agent is asking to be allowed
// something, the answer is right here. The terminal itself is one tap away.

import {
  Activity,
  type AgentView,
  Answer_Choice,
  BackgroundSession_State,
  type Question,
  type Turn,
  Turn_Kind,
} from '@ket/remote';
import * as Clipboard from 'expo-clipboard';
import { router, useFocusEffect, useLocalSearchParams, useNavigation } from 'expo-router';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import {
  AppState,
  Linking,
  Platform,
  Pressable,
  ScrollView,
  type StyleProp,
  StyleSheet,
  Text,
  TextInput,
  type TextInputInstance,
  type TextStyle,
  View,
  type ViewInstance,
  type ViewStyle,
} from 'react-native';
import { KeyboardChatScrollView } from 'react-native-keyboard-controller';
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import Svg, { Defs, LinearGradient, Rect, Stop as GradientStop } from 'react-native-svg';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { AttachMenu, FileChip, ImagesChip, SentFiles, type Source, Tray } from '../../../components/Attachments';
import { AgentViewList, backgroundState, terminalAt } from '../../../components/AgentView';
import { Outgoing, StatusMark, Working } from '../../../components/ChatRows';
import { CommandMenu } from '../../../components/Commands';
import { ComposerMenu, PlusButton, SendButton, plusMenu, slashMenu } from '../../../components/Composer';
import { FloatMenu, type Place, placeOf } from '../../../components/FloatMenu';
import { KeyboardLift } from '../../../components/KeyboardLift';
import { ModelSheet } from '../../../components/ModelSheet';
import { KeyCommands } from '../../../components/KeyCommands';
import { MicButton } from '../../../components/Mic';
import { RiskBadge } from '../../../components/Risk';
import { Dim, Tucked, type Tuckable, useTuck } from '../../../components/Tuck';
import { AgentMark, Bolt, Bookmark, Changes, Check, Chevron, ChevronDown, Close, Pause, Pencil, Shield, Sliders, TerminalWindow } from '../../../components/icons';
import { Bar, Button, Choice, Dot, IconCluster, Prose, Screen, field } from '../../../components/ui';
import { detail, everyWorktree, stateOf } from '../../../lib/activity';
import {
  type Attachment,
  MAX_BYTES,
  MAX_FILES,
  canPaste,
  fileName,
  fromCamera,
  fromClipboard,
  fromFiles,
  fromPhotos,
  splitFiles,
} from '../../../lib/attach';
import { type Command, commandsFor, matching } from '../../../lib/commands';
import { DesktopBusy, type PromptAnswer } from '../../../lib/connection';
import { useHost } from '../../../lib/hosts';
import type { Seed } from '../../../lib/starting';
import { useDictation } from '../../../lib/dictation';
import { confirmIdentity } from '../../../lib/lock';
import { Refused } from '../../../lib/refused';
import { agentLabel, blend, font, size, t, tint } from '../../../lib/theme';
import { canDictate } from '../../../lib/voice';

/** How often the conversation is asked for while it is on screen: more
 * often while the agent is working or a message is on its way. */
const POLL_MS = 1500;
const POLL_BUSY_MS = 700;

/** The pause between a message and its Enter — see the terminal screen. */
const ENTER_AFTER_MS = 80;

/** How long a sent message may go unseen — not in the conversation, not
 * waiting in the agent's queue — before it is shown as not delivered. */
const UNSEEN_MS = 15_000;

/** What the desktop says of a conversation while the agent is still opening
 * in its tab — see `conversation` in ket-core's phones/requests.rs. Said
 * while the task a chat was started on is on its way, they are that
 * message's sending; any other reason — the terminal closed, a transcript it
 * can't read — is news, and shown. */
const STILL_OPENING = ['no agent has reported from this terminal yet', "the session's transcript isn't there yet"];

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
  /** The photos and files sent with it. */
  files: Attachment[];
};

/** A message of yours as the conversation reads it back: its words, without
 * the paths the desktop listed under them, and how many images it carried. */
type Said = { words: string; images: number };

/** A card over the conversation's foot while something waits on you: a
 * background session's question, answered in agent view on the Mac; a
 * question or a plan; or a permission, with the answer just given here. */
type WaitingCard = Tuckable &
  (
    | { kind: 'background' }
    | { kind: 'ask' | 'plan'; question: Question }
    | { kind: 'permission'; question: Question; chosen?: Answer_Choice }
  );

/** The photos and files messages from here carried, by host, terminal and
 * message, while the app runs: the conversation read back has only their
 * count, so a message shows its pictures for as long as the phone has them. */
const carriedFiles = new Map<string, Attachment[]>();
const carriedKey = (where: string, said: Said) => `${where}\n${said.images}\n${said.words}`;

/** How tall the fade over the conversation's foot is, and how long it
 * takes to come and go. */
const EDGE_H = 28;
const EDGE_MS = 180;

/** Whether a card waiting on you floats over the conversation's foot, which
 * scrolls clear of it. Not on the web, whose scroll view has no content
 * inset to keep clear with: there the card sits in line over the composer. */
const FLOATS = Platform.OS !== 'web';

/** One-tap replies every session has. Chips over the composer while the
 * draft is empty, after `/` and the desktop's snippets. */
const REPLIES = ['Continue', 'Looks good', 'Run the tests', 'Commit it'];

/** What each conversation last showed, by host and terminal, while the app
 * runs: coming back to a chat draws it at once and reads on from where it
 * was, rather than starting over at "Loading the conversation…". */
const kept = new Map<string, { turns: Turn[]; cursor: bigint; session: string }>();

/** How long the conversation waits for the push to finish before painting
 * anyway: a screen that arrived without an animation, or a platform that
 * never says one ended, is not left blank. Longer than the stack's slide. */
const SETTLE_BOUND_MS = 600;

/** Each agent view session's name, by chat, for its title while agent view
 * isn't listing it — a cold start, or the listing being read again. */
const sessionNames = new Map<string, string>();

/** A terminal's chat — or, while the terminal shows Claude Code's agent
 * view, the background sessions it holds, each of which opens as a chat of
 * its own at `agent-session/[host]/[terminal]/[session]`. See AgentViewList.
 *
 * The list, once agent view is seen in the terminal, stays the list: agent
 * view is read off the Mac's screen, and a session open in it can look like
 * no agent view at all, which put a chat with no session under one opened
 * from the list. The chat gives way to the list when agent view appears —
 * someone opened it on the Mac, or it was still drawing when the chat was
 * opened — unless `?chat=1` asked for the chat itself. */
export default function Session() {
  const { host: hostId, terminal, chat } = useLocalSearchParams<{ host: string; terminal: string; chat?: string }>();
  const conn = useHost(hostId);
  const snapshot = conn?.snapshot ?? null;
  const at = terminalAt(snapshot, BigInt(terminal));
  const [mode, setMode] = useState<'list' | 'chat' | undefined>(chat ? 'chat' : undefined);
  const [last, setLast] = useState<AgentView | undefined>(undefined);
  const showing = at?.agentView;
  // Set while rendering, as React has state follow what it is drawn from.
  if (showing && showing !== last) setLast(showing);
  if (mode === undefined && snapshot) setMode(showing ? 'list' : 'chat');
  if (mode === 'chat' && showing && !chat) setMode('list');
  if (mode === undefined && !snapshot) {
    return (
      <Screen>
        <Bar back title="Session" />
      </Screen>
    );
  }
  const view = showing ?? last;
  if (mode === 'chat' || (mode === undefined && !showing) || !view || at?.closed) return <Chat />;
  return (
    <AgentViewList
      hostId={hostId}
      terminal={terminal}
      view={view}
      reading={!view.listed && (conn?.hostMinor ?? 0) >= 22}
      left={!showing}
    />
  );
}

/** The chat for a terminal's agent, or — with a `session` param — for one of
 * the background sessions its agent view holds.
 *
 * Or, `started`, the chat for work this phone just asked for, drawn in place
 * by the screen that stood in for it while the desktop made the worktree —
 * see app/starting. Starting a task moves you once, to that screen; when the
 * agent's tab opens the chat becomes this terminal's where it is, the task on
 * its way from the first frame and the draft typed there in its field. */
export function Chat({ started }: { started?: { host: string; terminal: string; seed?: Seed } } = {}) {
  const params = useLocalSearchParams<{ host: string; terminal: string; session?: string }>();
  const hostId = started?.host ?? params.host;
  const terminal = started?.terminal ?? params.terminal;
  const background = started ? '' : (params.session ?? '');
  const terminalId = BigInt(terminal);
  const conn = useHost(hostId);
  const insets = useSafeAreaInsets();
  // The stack renders this screen before it slides it in, and painting a
  // kept conversation — every turn, unvirtualised — on that first frame held
  // the tap for as long as the paint took. So the turns wait for the
  // transition to end; the header and composer come up at once. Drawn in
  // place, nothing slides in: painted at once.
  const navigation = useNavigation<{ addListener(type: 'transitionEnd', listener: () => void): () => void }>();
  const [settled, setSettled] = useState(started !== undefined);
  useEffect(() => {
    if (settled) return;
    let done = false;
    const settle = () => {
      if (done) return;
      done = true;
      setSettled(true);
    };
    const off = navigation.addListener('transitionEnd', settle);
    const bound = setTimeout(settle, SETTLE_BOUND_MS);
    return () => {
      off();
      clearTimeout(bound);
    };
  }, [navigation, settled]);
  const keptKey = background ? `${hostId}:${terminal}:${background}` : `${hostId}:${terminal}`;
  const [turns, setTurns] = useState<Turn[]>(() => kept.get(keptKey)?.turns ?? []);
  // The task a new worktree was started on from this phone: see `started`.
  const seed = started?.seed;
  // Sent from here and not in the conversation yet: shown at once, as
  // sending, then as queued while the agent's queue holds it.
  const [pending, setPending] = useState<Pending[]>(() =>
    seed ? [{ key: 0, text: seed.text, at: Date.now(), behind: false, files: [] }] : [],
  );
  const [queued, setQueued] = useState<string[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const sentCount = useRef(seed ? 1 : 0);
  const [unreadable, setUnreadable] = useState('');
  // Whether the desktop has answered once, and why it has not when asking
  // keeps failing: an empty conversation, one still loading and one the
  // desktop is refusing looked the same — blank.
  const [loaded, setLoaded] = useState(() => kept.has(keptKey));
  const [failing, setFailing] = useState('');
  const [message, setMessage] = useState(seed?.draft ?? '');
  // The draft as it is this moment, for what reads it after a wait: Send
  // after the mic's last words, which arrive between renders.
  const draft = useRef(seed?.draft ?? '');
  const write = (text: string) => {
    draft.current = text;
    setMessage(text);
  };
  const [note, setNote] = useState('');
  // The note is about a permission that is off: Settings, under it.
  const [toSettings, setToSettings] = useState(false);
  // Photos and files for the next message, in the field above its words.
  const [files, setFiles] = useState<Attachment[]>([]);
  // The `/` chip's menu — see `menu` below.
  const [listing, setListing] = useState(false);
  // The + menu, open in place of the `/` one, and whether it offers a paste.
  const [attaching, setAttaching] = useState(false);
  const [pasteable, setPasteable] = useState(false);
  // Where the cursor was in the draft, which is where dictation writes.
  const selection = useRef({ start: 0, end: 0 });
  const [answering, setAnswering] = useState(false);
  // The permission just answered, by its id, and how: its card goes on the
  // tap, the button tapped wearing its mark, and stays gone until the
  // desktop's next snapshot takes the question away. Back only if the
  // desktop would not take the answer, with its reason.
  const [answered, setAnswered] = useState<{ id: bigint; choice: Answer_Choice } | null>(null);
  // A Stop just pressed: the agent has the interrupt, and a second press
  // before it has stopped would reach whatever comes after.
  const [stopping, setStopping] = useState(false);
  // Sends so far: each one launches the arrow on the Send button.
  const [launches, setLaunches] = useState(0);
  // The agent's mode, from the pill in the bar: its menu, and the mode
  // being switched to while the desktop presses for it.
  const [modeMenu, setModeMenu] = useState<Place | null>(null);
  const [switching, setSwitching] = useState('');
  // How the last pick went, said in the sheet that made it: the chat's own
  // note is under the sheet while it is up.
  const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
  // Picks the desktop has made, each with what the snapshot said when it was
  // picked: shown in its place until the snapshot moves off that — to the
  // pick, a second or so later — rather than flicking back in between.
  const [landed, setLanded] = useState<Landed | null>(null);
  // The session sheet — mode, model and effort — for an agent with models (1.26).
  const [sheet, setSheet] = useState(false);
  const modePill = useRef<ViewInstance>(null);
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
  const input = useRef<TextInputInstance>(null);
  // Just cleared by Send, for a moment — see `SETTLE_MS`.
  const settling = useRef(false);
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
  // One of the terminal's agent view sessions: its own state, not the tab's.
  const backgroundSession = background
    ? located?.terminal?.agentView?.sessions.find((s) => s.session === background)
    : undefined;
  if (backgroundSession) sessionNames.set(keptKey, backgroundSession.name);
  const state = background
    ? backgroundState(backgroundSession?.state)
    : worktree
      ? stateOf(worktree.activity)
      : undefined;
  // Working, as the desktop's sidebar sees it: the agent thinks for a while
  // before anything reaches its transcript, and this says so meanwhile.
  const working = background
    ? backgroundSession?.state === BackgroundSession_State.WORKING
    : worktree?.activity === Activity.WORKING || worktree?.activity === Activity.RUNNING;
  const busy = useRef(false);
  const isBusy = working || pending.length > 0 || queued.length > 0;
  useEffect(() => {
    busy.current = isBusy;
  }, [isBusy]);
  // This terminal's own prompt. Several agents can run in one worktree, and
  // `question` is only the newest of their prompts; since 1.13 the desktop
  // sends every one, so the chat for this terminal shows its own.
  // A background session's prompts reach the desktop through agent view,
  // not this terminal's hooks: they are answered in the terminal.
  const question =
    !background && worktree?.activity === Activity.BLOCKED
      ? ([...worktree.questions, worktree.question].find((q) => q?.terminalId === terminalId) ?? undefined)
      : undefined;
  // A question or a plan rather than a permission: its own card, answerable
  // or not, in place of the permission's.
  const prompt = question ? promptKind(question.tool) : undefined;
  // A background session asking something: answered in agent view on the
  // Mac, where a message from here would land in its prompt.
  const waiting = !!background && backgroundSession?.state === BackgroundSession_State.NEEDS_YOU;
  const asking = !!question && (question.answerable || prompt !== undefined);
  // The card over the conversation's foot while one of those waits on you.
  const card = ((): WaitingCard | undefined => {
    if (waiting) return { key: 'background', kind: 'background' };
    if (!question) return undefined;
    const key = `q${question.id}`;
    if (prompt) return { key, kind: prompt, question };
    if (!question.answerable) return undefined;
    const chosen = answered?.id === question.id ? answered.choice : undefined;
    return { key, kind: 'permission', question, chosen, marked: chosen !== undefined };
  })();
  // The height it takes there, which the conversation scrolls clear of.
  const room = useSharedValue(0);
  const tuck = useTuck(card, room);
  // No chips while a question is asked, nor while its card tucks away: they
  // would come in under it and move it mid-slide.
  const quiet = asking || (tuck.card !== undefined && tuck.card.kind !== 'background');

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
          const reply = await conn.conversation(terminalId, cursor.current, background);
          if (stopped) return;
          failures = 0;
          setLoaded(true);
          setFailing('');
          if (!reply.supported) {
            // A chat already on screen stays as it is: one read the desktop
            // couldn't make is not news, and said mid-chat it read as
            // though the conversation were gone.
            if (session.current === '') setUnreadable(reply.reason);
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
          // After a fresh read, only the newest few can be one just sent. One
          // with photos keeps them, now it is delivered.
          const arrived = (restart ? reply.turns.slice(-6) : reply.turns)
            .filter((turn) => turn.kind === Turn_Kind.USER)
            .map(saidIn);
          if (arrived.length > 0) {
            setPending((before) => {
              const left = [...arrived];
              return before.filter((p) => {
                const at = left.findIndex((said) => fits(said, p));
                if (at < 0) return true;
                if (p.files.length > 0) carriedFiles.set(carriedKey(keptKey, left[at]), p.files);
                left.splice(at, 1);
                return false;
              });
            });
          }
          setNow(Date.now());
        } catch (e) {
          // Offline for a moment, or the desktop busy: the next tick asks
          // again, and a run of failures says why rather than staying blank.
          // A desktop turning a read away for a moment isn't failing: the
          // next tick is soon enough, and saying so would only alarm.
          if (e instanceof DesktopBusy) return;
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
    }, [conn, terminalId, tooOld, keptKey, background]),
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

  // The mic: see lib/dictation.ts. A message to a local agent keeps its
  // words on the phone too, or is refused.
  const local = /^(opencode:ollama\/|ollama:)/i.test(located?.terminal?.agent ?? worktree?.agent ?? '');
  const dictation = useDictation({
    local,
    draft,
    selection,
    write,
    onNote: (text, settings) => {
      setNote(text);
      setToSettings(settings);
    },
  });
  const hearing = dictation.hearing;

  // Typing needs the terminal's control, taken the first time and kept.
  // Pressing Send is asking to type, so control the desktop took back since
  // is taken again on the same press — asking for a second press made every
  // first message after using the Mac look lost. Once: a second refusal is
  // the desktop typing right now, and says so. `act` is `false` when the
  // phone turns out not to have control. Why not goes to `say`.
  const controlled = async (
    act: () => Promise<boolean>,
    failed: (why: string) => string,
    say: (text: string) => void = setNote,
  ): Promise<boolean> => {
    if (!conn) return false;
    for (let attempt = 0; attempt < 2; attempt += 1) {
      if (!held.current) {
        held.current = await conn.takeControl(terminalId).catch(() => false);
        if (!held.current) {
          say('Another device is typing in this terminal.');
          return false;
        }
      }
      try {
        if (await act()) return true;
        held.current = false;
      } catch (e) {
        say(failed(e instanceof Error ? e.message : String(e)));
        return false;
      }
    }
    say('Your desktop took control back.');
    return false;
  };
  const type = (text: string) => controlled(() => conn?.type(terminalId, text) ?? Promise.resolve(false), (why) => `Not sent: ${why}`);
  // A message for an agent view session: the desktop opens that session in
  // agent view first, then types it, so it never lands in whichever session
  // agent view had up. Empty text only opens it.
  // Reading one drives nothing on the Mac: only a message does, and the
  // terminal is let go once it is in, so the desktop and other phones keep it.
  const toBackground = async (text: string) => {
    const sent = await controlled(
      () => conn?.openAgentSession(terminalId, background, text) ?? Promise.resolve(false),
      (why) => `Not sent: ${why}`,
    );
    if (conn && held.current) {
      conn.releaseControl(terminalId);
      held.current = false;
    }
    return sent;
  };


  // The text, then Enter on its own, as the terminal screen sends it: sent
  // together, an agent reads the Enter as a new line in a paste. With photos
  // or files, the desktop types both once it has handed them over. Send while
  // dictating closes the mic and waits for its last words first.
  const send = async (said?: string) => {
    if (said === undefined) await dictation.finish();
    if (waiting) {
      setNote('It’s waiting on an answer in agent view on your Mac. Answer it there first.');
      return;
    }
    const text = (said ?? draft.current).trim();
    const attached = said === undefined && !background ? files : [];
    if (!text && attached.length === 0) return;
    setLaunches((n) => n + 1);
    if (said === undefined) {
      write('');
      input.current?.clear();
      settling.current = true;
      setTimeout(() => {
        settling.current = false;
      }, SETTLE_MS);
      setFiles([]);
    }
    setNote('');
    setToSettings(false);
    setAttaching(false);
    const key = ++sentCount.current;
    setPending((before) => [...before, { key, text, at: Date.now(), behind: working, files: attached }]);
    atEnd.current = true;
    const sent = background
      ? await toBackground(text)
      : attached.length > 0
        ? await controlled(
            () => conn?.sendFiles(terminalId, text, attached) ?? Promise.resolve(false),
            (why) => `Not sent: ${why}. Everything is back in the field; send again when it’s connected.`,
          )
        : await type(text);
    if (!sent) {
      setPending((before) => before.filter((p) => p.key !== key));
      if (said === undefined) {
        write(text);
        setFiles(attached);
      }
      return;
    }
    if (attached.length === 0 && !background) {
      await new Promise((resolve) => setTimeout(resolve, ENTER_AFTER_MS));
      await type('\r');
    }
    atEnd.current = true;
  };


  // The + menu's choices: photos, the camera, files, or what was copied, up
  // to what one message carries.
  const openAttach = () => {
    if (attaching) {
      setAttaching(false);
      return;
    }
    setListing(false);
    setAttaching(true);
    void canPaste().then(setPasteable);
  };
  const attach = async (source: Source) => {
    setAttaching(false);
    setNote('');
    setToSettings(false);
    const room = MAX_FILES - files.length;
    if (room <= 0) {
      setNote(`A message carries ${MAX_FILES} photos and files at most. Send these, then the rest.`);
      return;
    }
    try {
      const picked =
        source === 'photos'
          ? await fromPhotos(room)
          : source === 'camera'
            ? await fromCamera()
            : source === 'files'
              ? await fromFiles(room)
              : await fromClipboard();
      if (picked.length === 0) return;
      const all = [...files, ...picked].slice(0, MAX_FILES);
      if (all.reduce((sum, file) => sum + file.bytes.length, 0) > MAX_BYTES) {
        setNote('That’s more than one message carries, 15 MB. Send what’s attached first, then the rest.');
        return;
      }
      setFiles(all);
    } catch (e) {
      const why = e instanceof Error ? e.message : String(e);
      setNote(e instanceof Refused ? why : `Couldn’t attach it: ${why}`);
      setToSettings(e instanceof Refused);
    }
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
    // Gone on the tap: a second tap on a card still up was a second key.
    setAnswered({ id: question.id, choice });
    const why = await conn.answer(question, choice).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    if (why) {
      setAnswered(null);
      setNote(why);
    }
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
  // A card waiting on you, as `tuck` has it: the one asked, or the last one
  // as it tucks away.
  const drawCard = (c: WaitingCard) => {
    if (c.kind === 'background') {
      return (
        <View style={styles.ask}>
          <Text style={styles.askTitle} numberOfLines={1}>
            Waiting on you
          </Text>
          <Text style={styles.askLead} numberOfLines={3}>
            It’s asking something in agent view on your Mac. Answer it there; a message from here would land in its
            prompt.
          </Text>
          <Button kind="primary" title="Answer in the terminal" onPress={openTerminal} />
        </View>
      );
    }
    if (c.kind !== 'permission') {
      return (
        <Asking key={c.key} question={c.question} kind={c.kind} busy={answering} onRespond={respond} onTerminal={openTerminal} />
      );
    }
    // Answered: the button tapped wears its mark, the rest are out of reach.
    const done = c.chosen !== undefined;
    const markOf = (choice: Answer_Choice, color: string) =>
      c.chosen !== choice ? undefined : choice === Answer_Choice.DENY ? (
        <Close size={20} color={color} weight={2.4} />
      ) : (
        <Check size={20} color={color} weight={2.4} />
      );
    return (
      <View style={styles.ask}>
        <Dim on={done} style={styles.askBody}>
          <Text style={styles.askTitle} numberOfLines={1}>
            Wants to use {c.question.tool}
          </Text>
          <RiskBadge risk={c.question.risk} />
          {c.question.subject ? (
            <Text style={styles.askSubject} numberOfLines={4}>
              {c.question.tool === 'Bash' ? '$ ' : ''}
              {c.question.subject}
            </Text>
          ) : null}
        </Dim>
        <View style={styles.askButtons}>
          <Button
            kind="primary"
            title="Allow once"
            mark={markOf(Answer_Choice.ALLOW_ONCE, t.onAccent)}
            disabled={done}
            onPress={() => void answer(Answer_Choice.ALLOW_ONCE)}
            style={styles.grow}
          />
          {c.question.always ? (
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
      </View>
    );
  };
  // The agent's modes, as the desktop can tell them (1.24): Claude's
  // permission mode, OpenCode's agent, Codex's collaboration mode. Picking
  // one has the desktop press the agent's own key until it reports it —
  // a message asking it to implement cannot change its mode.
  const reported = background ? undefined : located?.terminal?.modes;
  // Its model and reasoning effort (1.26), Claude's and Codex's: read off
  // its transcript, changed by its own commands on the desktop.
  const told = background ? undefined : located?.terminal?.settings;
  // Each with a pick just made standing in until the snapshot moves. Retired
  // the moment it does — to the pick, or past it from the Mac — so it never
  // stands in again over a later change.
  const reporting: Record<keyof Landed, string | undefined> = {
    mode: reported?.current,
    model: told?.model,
    effort: told?.effort,
  };
  const picks = Object.entries(landed ?? {}) as [keyof Landed, { to: string; from: string }][];
  const standing = picks.filter(([what, pick]) => pick.from === reporting[what]);
  if (landed && standing.length < picks.length) {
    setLanded(standing.length > 0 ? Object.fromEntries(standing) : null);
  }
  const shown = (what: keyof Landed, value: string) => {
    const pick = landed?.[what];
    return pick && pick.from === value ? pick.to : value;
  };
  const modes = reported && landed?.mode ? { ...reported, current: shown('mode', reported.current) } : reported;
  const settings =
    told && (landed?.model || landed?.effort)
      ? { ...told, model: shown('model', told.model), effort: shown('effort', told.effort) }
      : told;
  // A desktop that never reports it — a session with no transcript yet —
  // has what it says shown again soon after.
  useEffect(() => {
    if (!landed) return;
    const timer = setTimeout(() => setLanded(null), 5000);
    return () => clearTimeout(timer);
  }, [landed]);
  const currentMode = modes?.choices.find((choice) => choice.id === modes.current);
  const modelLabel = settings?.models.find((choice) => choice.id === settings.model)?.label;
  const effortLabel = settings?.efforts.find((choice) => choice.id === settings.effort)?.label;
  const pillLabel = switching
    ? `${switching}…`
    : modelLabel
      ? effortLabel
        ? `${modelLabel} · ${effortLabel}`
        : modelLabel
      : (currentMode?.label ?? 'Mode');
  // A pick from the sheet or the mode menu: `act` has the desktop make it,
  // and `made` is what then stands in until the snapshot reports it.
  const pickWith = async (label: string, act: () => Promise<boolean>, made: Landed) => {
    if (switching || busy.current || conn?.status !== 'connected') return;
    setSwitching(label);
    setNote('');
    setSaid(null);
    try {
      const switched = await controlled(act, (why) => `Not switched: ${why}`, (text) => {
        setNote(text);
        setSaid({ text, failed: true });
      });
      if (switched) {
        setLanded((before) => ({ ...before, ...made }));
        setNote(`Switched to ${label}.`);
        setSaid({ text: `Switched to ${label}.`, failed: false });
      }
    } finally {
      setSwitching('');
    }
  };
  const pickModel = (model: string, effort: string, label: string) =>
    pickWith(
      label,
      () => conn?.setModel(terminalId, model, effort) ?? Promise.resolve(false),
      model ? { model: { to: model, from: told?.model ?? '' } } : { effort: { to: effort, from: told?.effort ?? '' } },
    );
  const pickMode = async (choice: { id: string; label: string }) => {
    setModeMenu(null);
    await pickWith(
      choice.label,
      () => conn?.setMode(terminalId, choice.id) ?? Promise.resolve(false),
      { mode: { to: choice.id, from: reported?.current ?? '' } },
    );
  };

  // Something to send: words, or photos and files on their own.
  const ready = !waiting && (message.trim() !== '' || files.length > 0);
  // A desktop from before 1.12 takes words and nothing else.
  const canAttach = conn === null || conn.hostMinor >= 12;

  // The `/` menu, while the draft is a slash and the start of a name.
  const commands = commandsFor(agent);
  const suggested = matching(commands, message);
  // The `/` chip's menu: every command, with nothing typed and the keyboard
  // left as it was. A draft that is a slash shows its own matches instead.
  const menu = listing && !message ? commands : suggested;
  const pick = async (command: Command) => {
    setListing(false);
    if (command.args) {
      write(`${command.name} `);
      input.current?.focus();
      return;
    }
    write('');
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
  const unsent = pending.filter((p) => !(p.text && queued.some((q) => carries(splitFiles(q).words, p.text))));
  // Nothing kept paints at once; a kept conversation waits for the slide.
  const painted = settled || turns.length === 0;
  // The task this chat was started on, not in the conversation yet: its
  // message, sending, is the whole chat until it is. The conversation still
  // loading, or the agent not yet reading — see STILL_OPENING — is that
  // message on its way, not news to put above it.
  const opening = seed !== undefined && pending.some((p) => p.key === 0);
  const statusOf = (p: Pending) => (now - p.at > UNSEEN_MS && !working ? 'unseen' : 'sending');
  const outgoing = (p: Pending) => (
    <Outgoing
      key={p.key}
      text={p.text}
      files={p.files}
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
          title={background ? (backgroundSession?.name ?? sessionNames.get(keptKey) ?? 'Session') : (worktree?.name ?? 'Session')}
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
              {modes || settings ? (
                <View ref={modePill} collapsable={false}>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={
                      settings
                        ? `${[currentMode?.label, modelLabel, effortLabel].filter(Boolean).join(', ') || 'Session settings'}. Change them`
                        : currentMode
                          ? `Mode: ${currentMode.label}. Change it`
                          : 'Mode. Change it'
                    }
                    disabled={conn?.status !== 'connected' || switching !== '' || (!settings && isBusy)}
                    onPress={() => {
                      if (settings) {
                        setSaid(null);
                        setSheet(true);
                      } else placeOf(modePill.current, setModeMenu);
                    }}
                    style={({ pressed }) => [styles.modePill, pressed && { backgroundColor: t.selection }]}
                  >
                    <ModeGlyph id={switching ? '' : (modes?.current ?? '')} />
                    <Text style={[styles.modePillText, settings && { maxWidth: 150 }]} numberOfLines={1}>
                      {pillLabel}
                    </Text>
                  </Pressable>
                </View>
              ) : null}
              {/* Where you can go from here, in one frame; the pill stands
                  alone because it changes the session rather than leaving it. */}
              <IconCluster
                actions={[
                  ...(worktree
                    ? [
                        {
                          label: 'The worktree',
                          onPress: () => router.push(`/worktree/${hostId}/${encodeURIComponent(worktree.id)}`),
                          icon: <Changes size={18} color={t.text} />,
                        },
                      ]
                    : []),
                  { label: 'Open the terminal', onPress: openTerminal, icon: <TerminalWindow size={18} color={t.text} /> },
                ]}
              />
            </View>
          }
        />
        <KeyboardChatScrollView
          ref={scroll}
          // Lifted with the keyboard, frame for frame, while the newest turn
          // is in view; someone scrolled up to read is left there.
          keyboardLiftBehavior="whenAtEnd"
          offset={lift}
          // A card waiting on you floats over the conversation's foot: the
          // conversation scrolls clear of it, and follows it down as it goes.
          extraContentPadding={FLOATS ? room : undefined}
          style={styles.fill}
          contentContainerStyle={styles.page}
          keyboardShouldPersistTaps="handled"
          onScroll={(e) => {
            const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
            // What is under a card waiting on you is not in view.
            const hidden = FLOATS ? room.get() : 0;
            const end = contentOffset.y + layoutMeasurement.height - hidden >= contentSize.height - 48;
            atEnd.current = end;
            if (end === away) setAway(!end);
            if (end && fresh) setFresh(false);
          }}
          scrollEventThrottle={100}
          // Shrunk by what grows under it — a taller draft, a note — while
          // the newest turn was in view: kept in view. A card waiting on you
          // floats over it instead (see FLOATS), kept clear by `room`.
          onLayout={() => {
            if (atEnd.current) scroll.current?.scrollToEnd({ animated: false });
          }}
          onContentSizeChange={() => {
            if (atEnd.current) scroll.current?.scrollToEnd({ animated: false });
          }}
        >
          {!painted ? null : tooOld ? (
            <View style={styles.empty}>
              <Status
                text={`ket on ${conn?.host.name ?? 'your Mac'} is older than this app. Update it — rebuild ket and start a new host — to read sessions here.`}
                onCopy={copy}
              />
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : unreadable && turns.length === 0 && !(opening && STILL_OPENING.includes(unreadable)) ? (
            <View style={styles.empty}>
              <Status text={sentence(unreadable)} onCopy={copy} />
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : turns.length === 0 && failing ? (
            <View style={styles.empty}>
              <Status text={`The desktop isn’t sending the conversation: ${failing}. Still trying.`} onCopy={copy} />
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : turns.length === 0 && !loaded && !opening ? (
            <View style={styles.empty}>
              <Prose>Loading the conversation…</Prose>
            </View>
          ) : turns.length === 0 && unsent.length === 0 && !working ? (
            <View style={styles.empty}>
              <Status text="Nothing in this session yet. What you and the agent say shows here." onCopy={copy} />
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : null}
          {grouped(painted ? turns : []).map(({ at, tools }) =>
            tools ? (
              <Tools key={at} turns={tools} onCopy={copy} />
            ) : (
              <TurnView
                key={at}
                turn={turns[at]}
                agent={agent}
                carried={turns[at].kind === Turn_Kind.USER ? carriedFiles.get(carriedKey(keptKey, saidIn(turns[at]))) : undefined}
                onCopy={copy}
                delivered={at === lastYours && unsent.length === 0 && queued.length === 0}
              />
            ),
          )}
          {/* A message that starts the agent working goes before the
              indicator: the desktop says the agent is working a moment
              before its transcript has the message that set it off. */}
          {unsent.filter((p) => !p.behind).map(outgoing)}
          {working || sending ? <Working agent={agent} what={working && worktree && !background ? detail(worktree) : ''} /> : null}
          {queued.map((text, index) => (
            <Outgoing key={`q${index}`} text={text} status="queued" onCopy={copy} />
          ))}
          {unsent.filter((p) => p.behind).map(outgoing)}
        </KeyboardChatScrollView>

        <KeyboardLift offset={{ closed: 0, opened: lift }}>
        {/* Over the conversation's foot, riding with the composer: a card
            waiting on you, and above it the fade and the jump button. */}
        <View pointerEvents="box-none" style={styles.foot}>
          <FadingEdge shown={away} />
          {away ? (
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

          {tuck.card ? <Tucked tuck={tuck}>{drawCard(tuck.card)}</Tucked> : null}
        </View>

        {hearing ? (
          <View accessibilityLiveRegion="polite" style={styles.hearing}>
            <View style={styles.hearingDot} />
            <Text style={styles.hearingText}>
              <Text style={{ color: t.text }}>Listening</Text>
              {hearing.onDevice
                ? ' · transcribed on this iPhone'
                : Platform.OS === 'web'
                  ? ' · transcribed by the browser'
                  : ' · transcribed by Apple'}
            </Text>
          </View>
        ) : note ? (
          <View style={styles.noteBox}>
            <Status text={note} onCopy={copy} style={styles.noteText} color={t.dim} />
            {toSettings ? (
              <Button title="Open Settings" onPress={() => void Linking.openSettings()} style={styles.settings} />
            ) : null}
          </View>
        ) : null}
        {/* Chips send at once, so not while photos wait to go with the next
            message, or while the mic is writing one. Under a menu they
            stay where they are: the menu floats over them. */}
        {!tooOld && !quiet && !message && files.length === 0 && !hearing ? (
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
                onPress={() => {
                  setAttaching(false);
                  setListing(!listing);
                }}
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
                  write(snippet.body);
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
        <KeyCommands title="Send" onSend={() => void send()} style={[styles.composer, { paddingBottom: rest }]}>
          {/* The + and / menus: floats hung from the composer's top edge,
              over the chat and the chips, which stay put under them. */}
          <ComposerMenu open={attaching} style={plusMenu}>
            <AttachMenu
              older={canAttach ? undefined : (conn?.host.name ?? 'your Mac')}
              pasteable={pasteable}
              onPick={(source) => void attach(source)}
            />
          </ComposerMenu>
          <ComposerMenu open={!attaching && menu.length > 0} style={slashMenu(!tooOld && !background)}>
            <CommandMenu commands={menu} onPick={(command) => void pick(command)} />
          </ComposerMenu>
          {!tooOld && !background ? <PlusButton open={attaching} onPress={openAttach} /> : null}
          <View style={[styles.field, files.length > 0 && styles.fieldTray, hearing && styles.fieldHearing]}>
            {files.length > 0 ? (
              <Tray files={files} onRemove={(key) => setFiles((before) => before.filter((file) => file.key !== key))} />
            ) : null}
            <View style={styles.fieldRow}>
              <TextInput
                ref={input}
                value={message}
                onChangeText={(text) => {
                  if (settling.current) {
                    input.current?.clear();
                    return;
                  }
                  dictation.letGo();
                  write(text);
                }}
                onSelectionChange={(e) => {
                  selection.current = e.nativeEvent.selection;
                }}
                placeholder={`Message ${agent ? agentLabel(agent) : 'the agent'}`}
                placeholderTextColor={t.faint}
                multiline
                style={[styles.input, !canDictate && styles.inputAlone, files.length > 0 && styles.inputUnderTray]}
              />
              {canDictate ? <MicButton
                  listening={hearing !== null}
                  level={dictation.level}
                  onPress={() => {
                    setAttaching(false);
                    setListing(false);
                    void dictation.toggle();
                  }}
                /> : null}
            </View>
          </View>
          {/* Working, with nothing to send: the same button stops the turn. */}
          <SendButton
            ready={ready}
            stop={working && !ready}
            stopping={stopping}
            launches={launches}
            onPress={
              working && !ready
                ? background
                  ? () => setNote('A background session stops in agent view on your Mac. Open the terminal to stop it.')
                  : stop
                : () => void send()
            }
          />
        </KeyCommands>
        </KeyboardLift>
        {settings ? (
          <ModelSheet
            open={sheet}
            onClose={() => setSheet(false)}
            agent={agent ? agentLabel(agent) : 'Agent'}
            modes={modes}
            settings={settings}
            pending={switching}
            said={said}
            busy={isBusy}
            onMode={(choice) => void pickMode(choice)}
            onModel={(choice) => void pickModel(choice.id, '', choice.label)}
            onEffort={(choice) => void pickModel('', choice.id, choice.label)}
          />
        ) : null}
        <FloatMenu
          open={modeMenu !== null}
          anchor={modeMenu}
          onClose={() => setModeMenu(null)}
          label={`${agent ? agentLabel(agent) : 'Agent'} mode`}
          sections={
            modes
              ? [
                  {
                    title: `${agent ? agentLabel(agent) : 'Agent'} mode`,
                    items: modes.choices.map((choice) => ({
                      key: choice.id,
                      label: choice.label,
                      detail: choice.detail || undefined,
                      icon: choice.id === modes.current ? <Check size={16} color={t.brand} weight={2.4} /> : undefined,
                      onPress: () => void pickMode(choice),
                    })),
                  },
                ]
              : []
          }
        />
      </View>
    </Screen>
  );
}

/** Picks the desktop has made and its snapshot hasn't reported yet: each
 * the value picked, and what the snapshot said when it was picked. */
type Landed = Partial<Record<'mode' | 'model' | 'effort', { to: string; from: string }>>;

/** The glyph a mode's pill leads with: pause for planning, a pencil for
 * edits going in, a shield for asking first, a bolt for going unasked. */
function ModeGlyph({ id }: { id: string }) {
  switch (id) {
    case 'plan':
      return <Pause size={15} color={t.attention} weight={2.6} />;
    case 'acceptEdits':
    case 'build':
      return <Pencil size={15} color={t.running} weight={2.2} />;
    case 'auto':
    case 'bypassPermissions':
    case 'dontAsk':
      return <Bolt size={15} color={t.brand} weight={2.2} />;
    case 'default':
      return <Shield size={15} color={t.dim} weight={2.2} />;
    default:
      return <Sliders size={15} color={t.dim} weight={2.2} />;
  }
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
  // Codex's own question form.
  if (tool === 'request_user_input') return 'ask';
  // A dialog the desktop read off the agent's screen, with its options —
  // Codex's "Implement this plan?" among them.
  if (tool === 'Dialog') return 'ask';
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
  // Sent: the button that sent it wears a tick until the card goes.
  const sentMark = sent ? <Check size={20} color={t.onAccent} weight={2.4} /> : undefined;

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
        <Dim on={sent} style={styles.askBody}>
          <Text style={styles.askTitle}>Ready to code?</Text>
          {question.subject ? (
            <Text style={styles.askLead} numberOfLines={2}>
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
          <View style={styles.askButtons}>
            <Button
              kind="primary"
              title="Send"
              mark={sentMark}
              disabled={!idle || !feedback.trim()}
              onPress={() => void send({ approve: false, feedback: feedback.trim() })}
              style={styles.grow}
            />
            <Button kind="ghost" title="Cancel" disabled={!idle} onPress={() => setRefining(false)} style={styles.grow} />
          </View>
        ) : (
          <View style={styles.askButtons}>
            <Button
              kind="primary"
              title="Approve"
              mark={sentMark}
              disabled={!idle}
              onPress={() => void send({ approve: true })}
              style={styles.grow}
            />
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
      <Dim on={sent}>
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
              {question.tool === 'Dialog' || question.tool === 'request_user_input' ? null : (
                // A dialog read off the agent's screen, or Codex's form, takes
                // one of its own options and nothing else: the desktop types
                // the option's number.
                <TextInput
                  value={others[i]}
                  onChangeText={(text) => write(i, text)}
                  placeholder="Or write your own"
                  placeholderTextColor={t.faint}
                  editable={idle}
                  style={field}
                />
              )}
            </View>
          ))}
        </ScrollView>
      </Dim>
      <View style={styles.askButtons}>
        <Button
          kind="primary"
          title={question.asks.length > 1 ? 'Send answers' : 'Send answer'}
          mark={sentMark}
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

/** A message of yours as the conversation reads it back. */
function saidIn(turn: Turn): Said {
  return { words: splitFiles(turn.text).words, images: turn.images ?? 0 };
}

/** Whether `said`, read back, is pending message `sent`: the same words —
 * see [`carries`] — or, for photos sent alone, images and no words. */
function fits(said: Said, sent: Pending): boolean {
  if (!sent.text) return said.words === '' && said.images > 0;
  return carries(said.words, sent.text);
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

/** A line about the chat rather than in it — why it is empty, what went
 * wrong — which copies when held, as a message does, so it can be sent on. */
function Status({
  text,
  onCopy,
  style,
  color,
}: {
  text: string;
  onCopy: (text: string) => void;
  style?: StyleProp<TextStyle>;
  color?: string;
}) {
  return (
    <Pressable
      accessibilityHint="Hold to copy"
      delayLongPress={350}
      onLongPress={() => onCopy(text)}
      style={({ pressed }) => (pressed ? styles.held : null)}
    >
      <Prose style={style} color={color}>
        {text}
      </Prose>
    </Pressable>
  );
}

/** A reason from the desktop as a sentence. */
function sentence(text: string): string {
  const s = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(s) ? s : `${s}.`;
}

/** The conversation as it is drawn: a run of two or more tool lines as one
 * group, every other turn on its own. Each is keyed by the turn it starts
 * at, so a group the agent is still adding to stays as open as it was. */
function grouped(turns: Turn[]): { at: number; tools?: Turn[] }[] {
  const items: { at: number; tools?: Turn[] }[] = [];
  let at = 0;
  while (at < turns.length) {
    let end = at;
    while (end < turns.length && turns[end].kind === Turn_Kind.TOOL) end += 1;
    if (end - at >= 2) {
      items.push({ at, tools: turns.slice(at, end) });
      at = end;
    } else {
      items.push({ at });
      at += 1;
    }
  }
  return items;
}

/** A run of tool lines, shut under how many there are and which tools: the
 * agent's words read straight through, and what it ran is a tap away — or
 * the whole of it, as it ran, in the terminal. */
function Tools({ turns, onCopy }: { turns: Turn[]; onCopy: (text: string) => void }) {
  const [open, setOpen] = useState(false);
  const count = `${turns.length} tool calls`;
  const names = [...new Set(turns.map((turn) => turn.tool).filter(Boolean))].join(', ');
  return (
    <View style={styles.tools}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={names ? `${count}: ${names}` : count}
        accessibilityState={{ expanded: open }}
        onPress={() => setOpen(!open)}
        style={({ pressed }) => [styles.toolsHead, pressed && styles.held]}
      >
        <View style={open && styles.toolsOpen}>
          <Chevron size={13} color={t.faint} weight={2} />
        </View>
        <Text style={styles.toolText} numberOfLines={1}>
          {count}
          {names ? <Text style={styles.toolNames}>{` · ${names}`}</Text> : null}
        </Text>
      </Pressable>
      {open ? turns.map((turn, index) => <TurnView key={index} turn={turn} agent="" onCopy={onCopy} />) : null}
    </View>
  );
}

function TurnView({
  turn,
  agent,
  carried,
  onCopy,
  delivered,
}: {
  turn: Turn;
  agent: string;
  /** The photos and files a message of yours carried, while the phone has
   * them. */
  carried?: Attachment[];
  onCopy: (text: string) => void;
  delivered?: boolean;
}) {
  // A tool line opened to its whole text.
  const [open, setOpen] = useState(false);
  switch (turn.kind) {
    case Turn_Kind.USER: {
      // Files the desktop named under the words, as files; images by their
      // count, unless the phone still has their pictures.
      const { words, files } = splitFiles(turn.text);
      const images = carried ? 0 : (turn.images ?? 0);
      const plain = !carried && images === 0 && files.length === 0;
      const said = words ? (
        <Text style={styles.youText}>
          <Linked text={words} />
        </Text>
      ) : null;
      const mark = delivered ? <StatusMark status="delivered" /> : null;
      return (
        <Pressable
          accessibilityHint="Hold to copy"
          delayLongPress={350}
          onLongPress={() => onCopy(words)}
          style={({ pressed }) => [plain ? styles.you : styles.yours, pressed && styles.held]}
        >
          {plain ? (
            <>
              {mark}
              {said}
            </>
          ) : (
            <>
              {carried ? <SentFiles files={carried} /> : files.map((path) => <FileChip key={path} name={fileName(path)} />)}
              {said || images > 0 ? (
                <View style={styles.bubble}>
                  {mark}
                  {images > 0 ? <ImagesChip count={images} /> : null}
                  {said}
                </View>
              ) : (
                mark
              )}
            </>
          )}
        </Pressable>
      );
    }
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
          {agent ? (
            <View style={styles.agentMark}>
              <AgentMark agent={agent} size={14} />
            </View>
          ) : null}
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

/** A message of yours: round all over but the corner nearest the composer,
 * which is tighter — a hint of where the message came from. An images chip
 * read back sits over its words. */
const bubble: ViewStyle = {
  backgroundColor: t.selection,
  borderRadius: 18,
  borderBottomRightRadius: 6,
  paddingHorizontal: 12,
  paddingVertical: 9,
  gap: 6,
};

const styles = StyleSheet.create({
  fill: { flex: 1 },
  page: { paddingHorizontal: size.gutter + 4, paddingTop: 4, paddingBottom: 18, gap: 12 },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: size.caption },
  barActions: { flexDirection: 'row', gap: 6, alignItems: 'center' },
  modePill: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 34,
    paddingLeft: 10,
    paddingRight: 12,
    borderRadius: 17,
    borderWidth: 1,
    borderColor: t.border,
    backgroundColor: t.sunken,
  },
  modePillText: { fontFamily: font.prose, fontSize: 13.5, fontWeight: '500', color: t.text, maxWidth: 104 },
  empty: { gap: 12, paddingVertical: 24, alignItems: 'flex-start' },
  you: { ...bubble, alignSelf: 'flex-end', maxWidth: '86%' },
  // A message of yours with photos or files: them, then its bubble.
  yours: { alignSelf: 'flex-end', maxWidth: '86%', alignItems: 'flex-end', gap: 4 },
  bubble,
  youText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 20, color: t.text },
  agent: { flexDirection: 'row', gap: 10, alignItems: 'flex-start' },
  // Centred on the first line of the words beside it — the body's 1pt top
  // padding plus a 21pt line — however many lines they run to.
  agentMark: { height: 22, paddingTop: 1, justifyContent: 'center' },
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
  tool: { flexDirection: 'row', alignItems: 'flex-start', gap: 8, paddingLeft: 24 },
  toolMark: { width: 5, height: 5, borderRadius: 3, marginTop: 6, backgroundColor: t.faint },
  toolText: { flex: 1, minWidth: 0, fontFamily: font.chrome, fontSize: size.detail, color: t.dim },
  tools: { gap: 10 },
  // The chevron centred where a tool line's dot is, its words where theirs start.
  toolsHead: { flexDirection: 'row', alignItems: 'center', gap: 4, paddingLeft: 20 },
  toolsOpen: { transform: [{ rotate: '90deg' }] },
  toolNames: { color: t.faint },
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
  askHeader: { fontFamily: font.chrome, fontSize: size.caption, color: t.attention },
  askScroll: { flexGrow: 0, maxHeight: 380 },
  askQuestions: { gap: 16 },
  asked: { gap: 6 },
  askInput: { height: undefined, minHeight: size.control, maxHeight: 120, paddingTop: 11, paddingBottom: 11, lineHeight: 20 },
  askButtons: { flexDirection: 'row', gap: 8 },
  askBody: { gap: 8 },
  grow: { flex: 1 },
  noteBox: { paddingHorizontal: size.gutter + 4, paddingBottom: 8, gap: 8, alignItems: 'flex-start' },
  noteText: { fontSize: size.label, lineHeight: 19 },
  settings: { height: 36, paddingHorizontal: 14 },
  // Dictating: a white dot and what is listening, where a note would be.
  hearing: { flexDirection: 'row', alignItems: 'center', gap: 7, paddingHorizontal: size.gutter + 4, paddingBottom: 8 },
  hearingDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: t.accent },
  hearingText: { fontFamily: font.prose, fontSize: size.label, lineHeight: 18, color: t.dim },
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
  // +, the field with the mic in it, and Send: everything else is a chip.
  composer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
    paddingHorizontal: size.gutter,
  },
  field: { flex: 1, minWidth: 0, minHeight: size.control, borderRadius: size.control / 2, backgroundColor: t.sunken },
  // With photos in it: a well, the tray over the words.
  fieldTray: { borderRadius: 20 },
  // Listening: a faint ring, inside, so nothing moves.
  fieldHearing: { boxShadow: `inset 0 0 0 1px ${tint(t.accent, 0.22)}` },
  fieldRow: { flexDirection: 'row', alignItems: 'flex-end' },
  input: {
    flex: 1,
    minWidth: 0,
    minHeight: size.control,
    maxHeight: 140,
    paddingLeft: 16,
    paddingRight: 0,
    paddingTop: 11,
    paddingBottom: 11,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
    lineHeight: 21,
    textAlignVertical: 'top',
  },
  // With no mic beside it, the words keep clear of the field's round end.
  inputAlone: { paddingRight: 16 },
  inputUnderTray: { paddingTop: 9 },
  edge: { position: 'absolute', top: -EDGE_H, left: 0, right: 0, height: EDGE_H },
  // Hung from the composer's top edge, as tall as the card in it: what
  // floats over the conversation's foot hangs from its top.
  foot: FLOATS ? { position: 'absolute', left: 0, right: 0, bottom: '100%' } : {},
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
