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
  type ColdCache,
  type Conversation,
  type Question,
  SessionLimit_Scope,
  type Turn,
  Turn_Kind,
} from '@ket/remote';
import * as Clipboard from 'expo-clipboard';
import { router, useFocusEffect, useIsFocused, useLocalSearchParams } from 'expo-router';
import { FlashList, type FlashListRef } from '@shopify/flash-list';
import {
  createContext,
  forwardRef,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  AppState,
  ScrollView,
  type ScrollViewProps,
  StyleSheet,
  Text,
  TextInput,
  type TextInputInstance,
  TouchableOpacity,
  View,
} from 'react-native';
import { KeyboardChatScrollView, type KeyboardChatScrollViewRef } from 'react-native-keyboard-controller';
import Animated, { type SharedValue, useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import Svg, { Defs, LinearGradient, Rect, Stop as GradientStop } from 'react-native-svg';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { attachSections, type Source, Tray } from '../../../components/Attachments';
import { AgentViewList, terminalAt } from '../../../components/AgentView';
import { CantFind, Compacting, Outgoing, type OutgoingStatus, Status, Working } from '../../../components/ChatRows';
import { ChatStart } from '../../../components/ChatStart';
import { ChatBar, ChatJobs, ChatNote, type Tone } from '../../../components/ChatBar';
import { JobsSheet } from '../../../components/JobsSheet';
import { Asking, cardStyles } from '../../../components/ChatCards';
import { Tools, TurnView } from '../../../components/ChatTurns';
import { commandRow } from '../../../components/Commands';
import { COMPOSER_GAP, Fold, PlusButton, SendButton, composerRest, plusMenu, slashMenu } from '../../../components/Composer';
import { FloatMenu } from '../../../components/FloatMenu';
import { KeyboardLift } from '../../../components/KeyboardLift';
import { KeyCommands } from '../../../components/KeyCommands';
import { type HandOver, LimitBar, LimitSheet, useLimitSheet, useStopped } from '../../../components/LimitSheet';
import { MicButton } from '../../../components/Mic';
import { RiskBadge } from '../../../components/Risk';
import { SessionPickers, useSessionPicks } from '../../../components/SessionPicks';
import { Dim, Tucked, type Tuckable, useTuck } from '../../../components/Tuck';
import {
  Bookmark,
  Check,
  ChevronDown,
  Close,
  Snowflake,
  TerminalWindow,
} from '../../../components/icons';
import { Bar, Button, Prose, Screen } from '../../../components/ui';
import { detail, everyWorktree, programBusy, promptKind } from '../../../lib/activity';
import { clockOf, minutesUntil, useBlackout } from '../../../lib/blackout';
import { useReadOnArrival } from '../../../lib/arrival';
import {
  type Attachment,
  canPaste,
  fromCamera,
  fromClipboard,
  fromFiles,
  fromPhotos,
  MAX_BYTES,
  MAX_FILES,
  splitFiles,
} from '../../../lib/attach';
import { type Command, commandsFor, matching } from '../../../lib/commands';
import { DesktopBusy, type HostConnection, type PromptAnswer, STARTING_FOR_MS } from '../../../lib/connection';
import { useHost, useHostState } from '../../../lib/hosts';
import { carries, fits, grouped, join, type Pending, type Said, saidIn } from '../../../lib/chat';
import { handChoices, limitKey, pickAgent } from '../../../lib/limits';
import { type Seed, reconcileTask, taskForTerminal } from '../../../lib/starting';
import { useDictation } from '../../../lib/dictation';
import { draftFor, keepDraft } from '../../../lib/drafts';
import { confirmIdentity } from '../../../lib/lock';
import { Refused } from '../../../lib/refused';
import { agentLabel, font, messagePlaceholder, size, t, tint } from '../../../lib/theme';
import { PRESSED, surface } from '../../../components/ui/surface';
import { canDictate } from '../../../lib/voice';
import { reasonOf, sentence } from '../../../lib/text';

/** How often the conversation is asked for while it is on screen: more
 * often while the agent is working or a message is on its way. */
const POLL_MS = 1500;
const POLL_BUSY_MS = 700;

/** A cursor that reads the conversation from its beginning. Out here
 * because the React Compiler skips a component with a BigInt literal in it. */
const FROM_THE_START = 0n;

/** The pause between a message and its Enter — see the terminal screen. */
const ENTER_AFTER_MS = 80;

/** How long a sent message may go unseen — not in the conversation, not
 * waiting in the agent's queue — before it is shown as not delivered. */
const UNSEEN_MS = 15_000;
/** How long the working row waits, once a message of yours is in the
 * conversation, for the desktop to say the agent is at work — see
 * `handingOver`. */
const HAND_OVER_MS = 8_000;

/** What the desktop says of a conversation while the agent is still opening
 * in its tab — see `conversation` in ket-core's phones/requests.rs. Said
 * while the task a chat was started on is on its way, they are that
 * message's sending; any other reason — the terminal closed, a transcript it
 * can't read — is news, and shown. */
/** The desktop's word for a session whose conversation it can't find. */
const NOT_THERE = "the session's transcript isn't there yet";
const STILL_OPENING = ['no agent has reported from this terminal yet', NOT_THERE];

/** How long after sending the composer turns away what iOS puts back:
 * Send tapped mid-autocorrect, the correction lands after the clearing. */
const SETTLE_MS = 300;

/** A card over the conversation's foot while something waits on you: a
 * background session's question, answered in agent view on the Mac; a
 * question or a plan; or a permission, with the answer just given here. */
type WaitingCard = Tuckable &
  (
    | { kind: 'background' }
    | { kind: 'cold'; cold: ColdCache }
    | { kind: 'ask' | 'plan'; question: Question }
    | { kind: 'permission'; question: Question; chosen?: Answer_Choice }
  );

/** The photos and files messages from here carried, by host, terminal and
 * message, while the app runs: the conversation read back has only their
 * count, so a message shows its pictures for as long as the phone has them.
 * Without their bytes, which went with the message: a picture is drawn from
 * its file, and every photo sent was otherwise held whole until the app
 * quit. */
const carriedFiles = new Map<string, Attachment[]>();
const SENT = new Uint8Array(0);
const carriedKey = (where: string, said: Said) => `${where}\n${said.images}\n${said.words}`;

/** Photos, one just taken, files, or what was copied: at most `room`. */
function pickFiles(source: Source, room: number): Promise<Attachment[]> {
  if (source === 'photos') return fromPhotos(room);
  if (source === 'camera') return fromCamera();
  if (source === 'files') return fromFiles(room);
  return fromClipboard();
}

/** How tall the fade over the conversation's foot is, and how long it
 * takes to come and go. */
const EDGE_H = 28;
const EDGE_MS = 180;

/** How far the desk under the composer reaches below it: see `styles.desk`. */
const DESK_BELOW = 48;

/** One-tap replies every session has. Chips over the composer while the
 * draft is empty, after `/` and the desktop's snippets. */
const REPLIES = ['Continue', 'Looks good', 'Run the tests', 'Commit it'];

/** What each conversation last showed, by host and terminal, while the app
 * runs: coming back to a chat draws it at once and reads on from where it
 * was, rather than starting over at "Loading the conversation…". Only the
 * KEPT_CHATS come to last: each is its conversation as read so far — the
 * desktop's latest turns as it opened, and every one since — and one for
 * every chat ever opened grew for as long as the app ran. One let go opens
 * as a first visit does. */
const kept = new Map<string, { turns: Turn[]; cursor: bigint; session: string }>();
const KEPT_CHATS = 12;

/** Keeps `chat` as `key`'s, the newest, letting go of the oldest past KEPT_CHATS. */
function keep(key: string, chat: { turns: Turn[]; cursor: bigint; session: string }): void {
  kept.delete(key);
  kept.set(key, chat);
  if (kept.size > KEPT_CHATS) kept.delete(kept.keys().next().value as string);
}

/** Tells the desktop the terminal's conversation is read, while this screen
 * is the one in front and its agent has an answer nobody has looked at. */
function useSeen(conn: HostConnection | null, terminalId: bigint, unread: boolean) {
  const showing = useIsFocused();
  useEffect(() => {
    if (conn && unread && showing) conn.markSeen(terminalId);
  }, [conn, unread, showing, terminalId]);
}

/** Cold spells already answered, by host, session and the request the spell
 * began after, while the app runs: each is asked about once (1.28). */
const coldAnswered = new Set<string>();

/** A composer's words and files on their way to the chat Start fresh moves
 * to, by host and terminal: taken once, by that chat as it opens. */
const carried = new Map<string, { draft: string; files: Attachment[] }>();

/** How far past the screen the conversation draws turns ahead of being
 * scrolled to, in points: room for the keyboard's lift, so a turn coming
 * into view as it rises is already there rather than drawn mid-slide. */
const DRAW_AHEAD = 600;

/** What the conversation's scroll view lifts by, from the chat drawing it:
 * see ChatScroll. */
const ChatScrollContext = createContext<{ lift: number; room?: SharedValue<number> }>({ lift: 0 });

/** The conversation's scroll view, under the list that draws only the turns
 * near the screen: lifted with the keyboard, frame for frame, while the
 * newest turn is in view, and someone scrolled up to read is left there. A
 * card waiting on you floats over the conversation's foot (`room`): the
 * conversation scrolls clear of it, and follows it down as it goes. One
 * component for the list's whole life, so the list never makes its scroll
 * view again; what changes comes through ChatScrollContext. */
const ChatScroll = forwardRef<KeyboardChatScrollViewRef, ScrollViewProps>(function ChatScroll(props, ref) {
  const { lift, room } = useContext(ChatScrollContext);
  return (
    <KeyboardChatScrollView
      ref={ref}
      keyboardLiftBehavior="whenAtEnd"
      offset={lift}
      extraContentPadding={room}
      automaticallyAdjustContentInsets={false}
      contentInsetAdjustmentBehavior="never"
      {...props}
    />
  );
});

/** A row of the conversation: a turn, or a run of tool lines by the turn
 * that starts it; and last, `FOOT`, what is on its way — messages sent from
 * here and the agent working. The foot is always there, so starting at the
 * last row starts at the end of everything. */
type Row = { key: string; at: number; tools?: Turn[] };

/** The foot row's `at`. */
const FOOT = -1;

const FOOT_ROW: Row = { key: 'foot', at: FOOT };

/** The space between rows, but not over the foot, which spaces itself only
 * while it holds something. */
function Gap({ trailingItem }: { trailingItem?: Row }) {
  return trailingItem?.at === FOOT ? null : <View style={styles.gap} />;
}

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
  // Held while another screen is on top: see `useHostState`.
  const hostState = useHostState(conn, useIsFocused());
  const snapshot = hostState?.snapshot ?? null;
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
      reading={!view.listed && (hostState?.hostMinor ?? 0) >= 22}
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
  // Held while another screen is on top, as its conversation is not asked
  // for then either: see `useHostState`.
  const hostState = useHostState(conn, useIsFocused());
  const insets = useSafeAreaInsets();
  const keptKey = background ? `${hostId}:${terminal}:${background}` : `${hostId}:${terminal}`;
  // The task a new worktree was started on from this phone: see `started`.
  // Before any state: the compiler can't tell a BigInt won't change, so
  // everything between `terminalId` and its last use here is held as still
  // changing, and a setter in that stretch is treated as one that does.
  const task = background ? undefined : taskForTerminal(hostState?.asked ?? [], terminalId);
  const [turns, setTurns] = useState<Turn[]>(() => kept.get(keptKey)?.turns ?? []);
  const seed = task?.delivery ? undefined : (started?.seed ?? (task ? { text: task.prompt, draft: '' } : undefined));
  // The normal hand-off starts a new delivery timer. Worktree creation and
  // agent startup may already have taken longer than that timer allows.
  const seedAt = started?.seed ? undefined : (task?.openedAt ?? task?.at);
  const delivered = (text: string) => turns.some((turn) => turn.kind === Turn_Kind.USER && carries(saidIn(turn).words, text));
  // Sent from here and not in the conversation yet: shown at once, as
  // sending, then as queued while the agent's queue holds it.
  const [pending, setPending] = useState<Pending[]>(() =>
    seed && !delivered(seed.text) ? [{ key: 0, text: seed.text, at: seedAt ?? Date.now(), behind: false, files: [] }] : [],
  );
  const [seeded, setSeeded] = useState(seed !== undefined);
  // useHost may resolve after the first render. Restore once, without
  // sending again, and let the transcript confirm delivery as usual.
  if (!seeded && seed) {
    setSeeded(true);
    if (!delivered(seed.text)) {
      setPending((before) => [{ key: 0, text: seed.text, at: seedAt ?? Date.now(), behind: false, files: [] }, ...before]);
    }
  }
  const [queued, setQueued] = useState<string[]>([]);
  // When the session began, over its first message; zero when not known.
  const [startedAt, setStartedAt] = useState(0);
  const [now, setNow] = useState(() => Date.now());
  // When a message sent from here reached the conversation before the
  // desktop said the agent was at work, and what the last draw saw of it.
  const [handedAt, setHandedAt] = useState(0);
  const [seen, setSeen] = useState({ yours: -1, sending: false });
  const sentCount = useRef(seed ? 1 : 0);
  const [unreadable, setUnreadable] = useState('');
  // Asks the desktop for the conversation now, not at the next tick.
  const lookNow = useRef(() => {});
  // Whether the desktop has answered once, and why it has not when asking
  // keeps failing: an empty conversation, one still loading and one the
  // desktop is refusing looked the same — blank.
  const [loaded, setLoaded] = useState(() => kept.has(keptKey));
  const [failing, setFailing] = useState('');
  // Whatever Start fresh carried here from the cold session's chat.
  const [carriedIn] = useState(() => {
    const brought = carried.get(keptKey);
    carried.delete(keptKey);
    return brought;
  });
  const [message, setMessage] = useState(carriedIn?.draft ?? seed?.draft ?? '');
  // The draft as it is this moment, for what reads it after a wait: Send
  // after the mic's last words, which arrive between renders.
  const draft = useRef(carriedIn?.draft ?? seed?.draft ?? '');
  // The worktree this chat's terminal is in, once the snapshot says: what
  // is typed here is kept on the phone under it (lib/drafts.ts).
  const draftWhere = useRef<string | undefined>(undefined);
  const write = (text: string) => {
    draft.current = text;
    setMessage(text);
    if (draftWhere.current !== undefined) keepDraft(keptKey, draftWhere.current, text);
  };
  // A line for the reader, in a capsule over the top of the conversation,
  // and what kind it is: done, copied, a heads-up, or by the way.
  const [notice, setNotice] = useState<{ text: string; tone: Tone }>({ text: '', tone: 'warn' });
  const note = notice.text;
  const setNote = (text: string, tone: Tone = 'warn') => setNotice({ text, tone });
  // The note is about a permission that is off: Settings, under it.
  const [toSettings, setToSettings] = useState(false);
  // Photos and files for the next message, in the field above its words.
  const [files, setFiles] = useState<Attachment[]>(carriedIn?.files ?? []);
  // The files as they are this moment, for what reads them after a wait:
  // Start fresh carries them to the fresh chat once it opens.
  const filesNow = useRef(files);
  useEffect(() => {
    filesNow.current = files;
  }, [files]);
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
  // A cold session’s card (1.28): redrawn once it is answered, and held
  // while its fresh session is on the way.
  const [, setColdTick] = useState(0);
  const [freshing, setFreshing] = useState(false);
  const [snippets, setSnippets] = useState<{ name: string; body: string }[]>([]);
  const cursor = useRef(kept.get(keptKey)?.cursor ?? FROM_THE_START);
  const session = useRef(kept.get(keptKey)?.session ?? '');
  // Kept with the cursor that produced it: a later read that moved the cursor
  // without new turns only leaves the kept cursor behind, over records that
  // make no turns, so reading on from it never repeats one.
  useEffect(() => {
    if (turns.length > 0) keep(keptKey, { turns, cursor: cursor.current, session: session.current });
  }, [keptKey, turns]);
  const held = useRef(false);
  // The conversation's list, for scrolls to its end (see `toEnd`).
  const list = useRef<FlashListRef<Row>>(null);
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

  const located = everyWorktree(hostState?.snapshot ?? null)
    .map((l) => ({ ...l, terminal: l.worktree.terminals.find((x) => x.id === terminalId) }))
    .find((l) => l.terminal);
  const worktree = located?.worktree;
  // The draft kept for this chat — left by a crash, an update or leaving it
  // — back in the field once its worktree is known, unless something is in
  // the field already, which is kept in its place. Before the first paint.
  const worktreeId = worktree?.id;
  useLayoutEffect(() => {
    if (worktreeId === undefined || draftWhere.current !== undefined) return;
    draftWhere.current = worktreeId;
    const text = draft.current || draftFor(keptKey, worktreeId);
    draft.current = text;
    setMessage(text);
    keepDraft(keptKey, worktreeId, text);
  }, [worktreeId, keptKey]);
  // One of the terminal's agent view sessions: its own state, not the tab's.
  const backgroundSession = background
    ? located?.terminal?.agentView?.sessions.find((s) => s.session === background)
    : undefined;
  if (backgroundSession) sessionNames.set(keptKey, backgroundSession.name);
  // Working, as the desktop's sidebar sees it: the agent thinks for a while
  // before anything reaches its transcript, and this says so meanwhile.
  const working = background
    ? backgroundSession?.state === BackgroundSession_State.WORKING
    : worktree?.activity === Activity.WORKING || (worktree?.activity === Activity.RUNNING && !programBusy(worktree));
  // Compacting its conversation (1.30): said above the composer, not in the
  // chat, and a message sent meanwhile is on its way rather than lost.
  const compacting = !background && located?.terminal?.compacting === true;
  const busy = useRef(false);
  const isBusy = working || compacting || pending.length > 0 || queued.length > 0;
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
  // An answer nobody has looked at is read once this screen shows it, and
  // again each time one lands while it is the screen in front (1.32).
  const unread = located?.terminal?.unread ?? false;
  useSeen(conn, terminalId, unread);
  // A worktree marked unread from its menu, read as its chat is come to (1.35).
  useReadOnArrival(conn, worktree);
  // A session whose prompt cache has gone cold since it last ran (1.28):
  // asked about once a spell, as the chat opens — never at Send, since a
  // message always goes as typed, and sending one is the answer Continue.
  const cold = !background && (hostState?.hostMinor ?? 0) >= 28 ? located?.terminal?.cold : undefined;
  const coldKey = cold ? `${hostId}:${cold.session}:${cold.sinceMs}` : '';
  const askCold = cold !== undefined && !coldAnswered.has(coldKey);
  const answerCold = () => {
    if (!coldKey || coldAnswered.has(coldKey)) return;
    coldAnswered.add(coldKey);
    setColdTick((n) => n + 1);
  };
  // A session its plan stopped (1.38): the composer gives way to a line
  // saying so, and a sheet over the chat — once a stop — says when it
  // resets and offers the agent ket would hand the task to.
  const stopped = useStopped(!background && (hostState?.hostMinor ?? 0) >= 38 ? located?.terminal?.limit : undefined);
  const limitSheet = useLimitSheet(stopped ? limitKey(hostId, terminalId, stopped) : '');
  const stoppedAgent = located?.terminal?.agent ?? '';
  // What its agent left running on the Mac (1.40): a capsule under the bar
  // while any of it runs, opening a sheet of it with each one's Stop.
  const jobs = !background && (hostState?.hostMinor ?? 0) >= 40 ? (located?.terminal?.jobs ?? []) : [];
  const runningJobs = jobs.filter((job) => !job.ended).length;
  const [jobsOpen, setJobsOpen] = useState(false);
  const handTo = stopped ? pickAgent(hostState?.snapshot ?? null, stoppedAgent) : undefined;
  // A stop that names no reset is never over here, and one model's limit is
  // lifted by changing model: the composer stays under its line, to try
  // again once the plan is topped up or the model changed.
  const composing = !stopped || !stopped.resetsAtMs || stopped.scope === SessionLimit_Scope.MODEL;
  // Handing the task over (1.39): the brief previewed, then sent with the
  // lease, and the chat follows the new agent's terminal.
  const canHand = !background && (hostState?.hostMinor ?? 0) >= 39;
  // The card over the conversation's foot while one of those waits on you.
  const card = ((): WaitingCard | undefined => {
    if (waiting) return { key: 'background', kind: 'background' };
    if (!question) return askCold && cold ? { key: `cold${cold.sinceMs}`, kind: 'cold', cold } : undefined;
    const key = `q${question.id}`;
    if (prompt) return { key, kind: prompt, question };
    // One the Mac cannot answer from here — it does not know that agent's
    // keys for it — is still a wait: said, with the way to the terminal,
    // rather than a chat that looks idle while the agent sits on a prompt.
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
  const tooOld = hostState !== null && hostState.status === 'connected' && hostState.hostMinor < 7;

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
      // What the desktop answered, taken in.
      const take = (reply: Conversation) => {
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
        const task = background ? undefined : taskForTerminal(conn.state.asked, terminalId);
        const taskSettled = task && reconcileTask(
          task,
          reply.session,
          reply.turns.some((turn) => turn.kind === Turn_Kind.USER && carries(saidIn(turn).words, task.prompt)),
          reply.turns.some((turn) => turn.kind === Turn_Kind.USER || turn.kind === Turn_Kind.TOOL),
        );
        const restart = reply.fresh || reply.session !== session.current;
        session.current = reply.session;
        cursor.current = reply.next;
        if (restart) setTurns(reply.turns);
        else if (reply.turns.length > 0) {
          setTurns((before) => join(before, reply.turns));
          if (!atEnd.current) setFresh(true);
        }
        setQueued(reply.queued);
        if (restart || Number(reply.startedAtMs) > 0) setStartedAt(Number(reply.startedAtMs));
        // A message that has reached the conversation is no longer pending.
        // After a fresh read, only the newest few can be one just sent. One
        // with photos keeps them, now it is delivered.
        const arrived = (restart ? reply.turns.slice(-6) : reply.turns)
          .filter((turn) => turn.kind === Turn_Kind.USER)
          .map(saidIn);
        if (reply.turns.length > 0 || taskSettled) {
          setPending((before) => {
            const left = [...arrived];
            return before.filter((p) => {
              if (p.key === 0 && taskSettled) return false;
              // A recovered task can be older than the newest six turns
              // used to match messages just sent from this chat.
              if (p.key === 0 && reply.turns.some((turn) => turn.kind === Turn_Kind.USER && carries(saidIn(turn).words, p.text))) return false;
              const at = left.findIndex((said) => fits(said, p));
              if (at < 0) return true;
              if (p.files.length > 0) {
                carriedFiles.set(carriedKey(keptKey, left[at]), p.files.map((file) => ({ ...file, bytes: SENT })));
              }
              left.splice(at, 1);
              return false;
            });
          });
        }
        setNow(Date.now());
      };
      // Offline for a moment, or the desktop busy: the next tick asks again,
      // and a run of failures says why rather than staying blank. A desktop
      // turning a read away for a moment isn't failing: the next tick is soon
      // enough, and saying so would only alarm.
      const fail = (e: unknown) => {
        if (e instanceof DesktopBusy) return;
        failures += 1;
        if (!stopped && failures >= 3) setFailing(reasonOf(e));
      };
      // Chained rather than in a try: the React Compiler skips a component
      // with a conditional inside a try. A reply that can't be taken in
      // counts as a failure too.
      const tick = async () => {
        if (asking) return;
        asking = true;
        await conn
          .conversation(terminalId, cursor.current, background)
          .then(take)
          .catch(fail)
          .finally(() => {
            asking = false;
          });
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
      lookNow.current = now;
      let wasConnected = conn.state.status === 'connected';
      const offChange = conn.onChange(() => {
        const connected = conn.state.status === 'connected';
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
    say?: (text: string) => void,
  ): Promise<boolean> => {
    const tell = say ?? ((text: string) => setNote(text));
    if (!conn) return false;
    for (let attempt = 0; attempt < 2; attempt += 1) {
      if (!held.current) {
        held.current = await conn.takeControl(terminalId).catch(() => false);
        if (!held.current) {
          tell('Another device is typing in this terminal.');
          return false;
        }
      }
      try {
        if (await act()) return true;
        held.current = false;
      } catch (e) {
        tell(failed(reasonOf(e)));
        return false;
      }
    }
    tell('Your desktop took control back.');
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
      setNote('It’s waiting on an answer in agent view on your Mac. Answer it there first.', 'info');
      return;
    }
    const text = (said ?? draft.current).trim();
    const attached = said === undefined && !background ? files : [];
    if (!text && attached.length === 0) return;
    const resetsChat = !background && attached.length === 0
      && commands.some((command) => command.resetsChat && command.name === text);
    // Sent as typed, cold or not: sending is the answer Continue.
    answerCold();
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
    setListing(false);
    const key = ++sentCount.current;
    if (!resetsChat) {
      setPending((before) => [...before, { key, text, at: Date.now(), behind: working, files: attached }]);
    }
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
    // The clipboard first, then the menu, opened with every row it will
    // have. Opened first, a chat's first + changed under it as the answer
    // came — a copied image adding Paste image — and drew nothing.
    void canPaste().then((has) => {
      setPasteable(has);
      setAttaching(true);
    });
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
    // No `?:` in here: the React Compiler skips a component with one in a try.
    try {
      const picked = await pickFiles(source, room);
      if (picked.length === 0) return;
      const all = [...files, ...picked].slice(0, MAX_FILES);
      if (all.reduce((sum, file) => sum + file.bytes.length, 0) > MAX_BYTES) {
        setNote('That’s more than one message carries, 15 MB. Send what’s attached first, then the rest.');
        return;
      }
      setFiles(all);
    } catch (e) {
      const why = reasonOf(e);
      const refused = e instanceof Refused;
      if (refused) setNote(why);
      else setNote(`Couldn’t attach it: ${why}`);
      setToSettings(refused);
    }
  };

  // Held on a message or a code block: its words, on the clipboard.
  // The same function every draw, so a turn already on screen is not drawn
  // again for a poll that brought nothing new to it (see TurnView).
  const copy = useCallback((text: string) => {
    void Clipboard.setStringAsync(text);
    setNotice({ text: 'Copied', tone: 'copied' });
    setTimeout(() => setNotice((shown) => (shown.text === 'Copied' ? { text: '', tone: 'warn' } : shown)), 1500);
  }, [setNotice]);

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
  // Failed: the agent ended on an error the conversation may never show,
  // and the header says no state. Said in the note as it happens, or as a
  // chat that has already failed opens; taken back once it is not so.
  const failed = !background && worktree?.activity === Activity.FAILED;
  const failure = `${agent ? agentLabel(agent) : 'The agent'} failed. Open the terminal to see why.`;
  const [toldFailed, setToldFailed] = useState(false);
  if (failed !== toldFailed) {
    setToldFailed(failed);
    if (failed) setNote(failure);
    else if (note === failure) setNote('');
  }
  // A blackout about to shut ket off on this phone: said once, ten minutes
  // ahead or as the chat opens inside them — see lib/blackout.ts.
  const blackout = useBlackout();
  const soon = blackout.soon;
  const [warned, setWarned] = useState(0);
  if (soon && soon.at !== warned) {
    setWarned(soon.at);
    setNote(`ket shuts off at ${clockOf(soon.at)}, in ${minutesUntil(soon.at, blackout.now)}.`, 'info');
  }
  // Under the composer at rest: clear of the home indicator rather than the
  // whole safe area. With the keyboard up the composer rides on it, 6pt
  // above, so it and the conversation are lifted by the keyboard less this.
  const rest = composerRest(insets.bottom);
  const lift = rest - COMPOSER_GAP;
  const openTerminal = () => router.push(`/terminal/${hostId}/${terminal}`);
  // A card waiting on you, as `tuck` has it: the one asked, or the last one
  // as it tucks away.
  // Start fresh: the desktop clears the cold session in place — `/clear` in
  // the same terminal — and the chat follows the terminal to its new
  // session. A desktop before ket 0.3 opens one beside it instead, and the
  // chat moves there.
  const onScreen = useRef(false);
  useFocusEffect(
    useCallback(() => {
      onScreen.current = true;
      return () => {
        onScreen.current = false;
      };
    }, []),
  );
  const startFresh = async (spell: ColdCache) => {
    if (!conn || freshing) return;
    setFreshing(true);
    // A message sent here while the fresh one starts goes in after the
    // desktop's `/clear`, to the new session. From a desktop before ket 0.3,
    // it goes to this session, as typed, and this chat stays on it.
    const sentBefore = sentCount.current;
    let started: bigint | undefined;
    await controlled(
      async () => {
        const outcome = await conn.startFresh(terminalId, spell.session, spell.sinceMs);
        if (outcome === false) return false;
        if ('why' in outcome) throw new Error(outcome.why);
        started = outcome.terminalId;
        return true;
      },
      (why) => `Not started fresh: ${why}`,
    );
    if (started === undefined) {
      setFreshing(false);
      return;
    }
    answerCold();
    // The same terminal: its next read is the new session's, and this chat
    // swaps to it where it stands.
    if (started === terminalId) {
      setFreshing(false);
      return;
    }
    await conn.awaitTerminal(started);
    setFreshing(false);
    // Only from this chat, still on screen: one left meanwhile is not to
    // pull the reader back, nor to replace whatever they went to.
    if (!onScreen.current) return;
    // The mic's last words land in the draft as it finishes, and leaving
    // this chat would abandon them.
    await dictation.finish();
    if (!onScreen.current) return;
    if (sentCount.current !== sentBefore) {
      setNote('Your message went to this session, so the chat stays on it. The fresh one is open beside it on your Mac.', 'info');
      return;
    }
    carried.set(`${hostId}:${started}`, { draft: draft.current, files: filesNow.current });
    // The draft goes with it, so this chat's is let go.
    if (draftWhere.current !== undefined) keepDraft(keptKey, draftWhere.current, '');
    router.replace(`/session/${hostId}/${started}`);
  };
  const hand: HandOver | undefined =
    stopped && canHand && conn
      ? {
          choices: handChoices(hostState?.snapshot ?? null, stoppedAgent),
          preview: (to, depth) => conn.previewSwitch(terminalId, to, depth),
          send: async (to, text, beside, session) => {
            let failed: string | undefined;
            let opened: bigint | undefined;
            await controlled(
              async () => {
                const outcome = await conn.switchAgent(terminalId, to, text, beside, session);
                if (outcome === false) return false;
                if ('why' in outcome) throw new Error(outcome.why);
                opened = outcome.terminalId;
                return true;
              },
              (why) => why,
              (said) => {
                failed = said;
              },
            );
            if (opened === undefined) return failed ?? 'It wasn’t handed over.';
            limitSheet.close();
            await conn.awaitTerminal(opened);
            if (!onScreen.current) return undefined;
            // What was typed or attached here, and not sent, goes along as
            // the new chat's unsent draft — not as part of the brief.
            await dictation.finish();
            if (!onScreen.current) return undefined;
            carried.set(`${hostId}:${opened.toString()}`, { draft: draft.current, files: filesNow.current });
            router.replace(`/session/${hostId}/${opened.toString()}`);
            return undefined;
          },
        }
      : undefined;
  const drawCard = (c: WaitingCard) => {
    if (c.kind === 'cold') {
      return (
        <View style={styles.cold}>
          <View style={styles.coldTitle}>
            <Snowflake size={18} color={t.dim} />
            <Text style={cardStyles.askTitle} numberOfLines={1}>
              This session went cold
            </Text>
          </View>
          <Text style={cardStyles.askLead}>
            {freshing ? 'Starting a fresh session on your Mac…' : `Continuing ${c.cold.summary}.`}
          </Text>
          <View style={cardStyles.askButtons}>
            <Button
              kind="secondary"
              title="Start fresh"
              style={cardStyles.grow}
              disabled={freshing}
              onPress={() => void startFresh(c.cold)}
            />
            <Button kind="primary" title="Continue" style={cardStyles.grow} disabled={freshing} onPress={answerCold} />
          </View>
        </View>
      );
    }
    if (c.kind === 'background') {
      return (
        <View style={cardStyles.ask}>
          <Text style={cardStyles.askTitle} numberOfLines={1}>
            Waiting on you
          </Text>
          <Text style={cardStyles.askLead} numberOfLines={3}>
            It’s asking something in agent view on your Mac. Answer it there; a message from here would land in its
            prompt.
          </Text>
          <Button
            kind="primary"
            title="Answer in the terminal"
            icon={<TerminalWindow size={16} color={t.onAccent} />}
            onPress={openTerminal}
          />
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
      <View style={cardStyles.ask}>
        <Dim on={done} style={cardStyles.askBody}>
          <Text style={cardStyles.askTitle} numberOfLines={1}>
            Wants to use {c.question.tool}
          </Text>
          <RiskBadge risk={c.question.risk} />
          {c.question.subject ? (
            <Text style={cardStyles.askSubject} numberOfLines={4}>
              {c.question.tool === 'Bash' ? '$ ' : ''}
              {c.question.subject}
            </Text>
          ) : null}
        </Dim>
        {c.question.answerable ? (
          <View style={cardStyles.askButtons}>
            <Button
              kind="primary"
              title="Allow once"
              mark={markOf(Answer_Choice.ALLOW_ONCE, t.onAccent)}
              disabled={done}
              onPress={() => void answer(Answer_Choice.ALLOW_ONCE)}
              style={cardStyles.grow}
            />
            {c.question.always ? (
              <Button
                title="Always"
                mark={markOf(Answer_Choice.ALLOW_ALWAYS, t.text)}
                disabled={done}
                onPress={() => void answer(Answer_Choice.ALLOW_ALWAYS)}
                style={cardStyles.grow}
              />
            ) : null}
            <Button
              kind="danger"
              title="Deny"
              mark={markOf(Answer_Choice.DENY, t.failed)}
              disabled={done}
              onPress={() => void answer(Answer_Choice.DENY)}
              style={cardStyles.grow}
            />
          </View>
        ) : (
          <Button
            kind="primary"
            title="Answer in the terminal"
            icon={<TerminalWindow size={16} color={t.onAccent} />}
            onPress={openTerminal}
          />
        )}
      </View>
    );
  };
  // The agent's mode, model and effort: the bar's pill and what it opens.
  const picks = useSessionPicks({
    conn,
    terminalId,
    // Not OpenCode's: the desktop tells its agent (Build, Plan) but not its
    // model, so its pill said "Build" where the others say their model.
    reported: background || /^opencode/i.test(located?.terminal?.agent ?? '') ? undefined : located?.terminal?.modes,
    told: background ? undefined : located?.terminal?.settings,
    busy,
    controlled,
    setNote,
  });

  // Something to send: words, or photos and files on their own.
  const ready = !waiting && (message.trim() !== '' || files.length > 0);
  // A desktop from before 1.12 takes words and nothing else.
  const canAttach = hostState === null || hostState.hostMinor >= 12;

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

  // The agent's queue, each message with the photos and files it was sent
  // with from here: the queue has only its words, and the paths the desktop
  // listed under them for one typed there.
  const left = [...pending];
  const inQueue = queued.map((text) => {
    const { words, files } = splitFiles(text);
    const at = left.findIndex((p) => (p.text ? carries(words, p.text) : !words && files.length > 0 && p.files.length > 0));
    return { words, named: files, files: at < 0 ? [] : left.splice(at, 1)[0].files };
  });
  // Sent from here and in neither the conversation nor the agent's queue yet.
  const unsent = left;
  // The rows, made again only when the turns change: the list redraws the
  // ones it has on screen, and only the ones that differ (see TurnView).
  const rows = useMemo<Row[]>(
    () => [...grouped(turns).map(({ at, tools }) => ({ key: `t${at.toString()}`, at, tools })), FOOT_ROW],
    [turns],
  );
  const scrollContext = useMemo(() => ({ lift, room }), [lift, room]);
  // Down to the very end, past the keyboard's lift and a card's room. The
  // list first brings its last row in — rows not drawn yet have only a
  // guessed height, and a scroll by those stops short — then has its scroll
  // view finish, which knows the keyboard and the card.
  const toEnd = (animated: boolean) => void list.current?.scrollToEnd({ animated });
  // The task this chat was started on, not in the conversation yet: its
  // message, sending, is the whole chat until it is. The conversation still
  // loading, or the agent not yet reading — see STILL_OPENING — is that
  // message on its way, not news to put above it.
  const opening = seed !== undefined && pending.some((p) => p.key === 0);
  // The conversation not found on the Mac: said in the middle of the chat,
  // with Try again and Open terminal.
  const missing = !tooOld && !opening && unreadable === NOT_THERE && turns.length === 0;
  const statusOf = (p: Pending): OutgoingStatus => {
    const age = now - p.at;
    if (age <= UNSEEN_MS) return 'sending';
    if (p.key === 0 && age <= STARTING_FOR_MS) return 'held';
    if (p.key === 0 && STILL_OPENING.includes(unreadable)) return 'unseen';
    return working || compacting ? 'sending' : 'unseen';
  };
  const outgoing = (p: Pending) => (
    <Outgoing
      key={p.key}
      text={p.text}
      files={p.files}
      status={statusOf(p)}
      onCopy={copy}
      onDismiss={() => {
        if (p.key === 0 && task) task.delivery = 'dismissed';
        setPending((before) => before.filter((x) => x.key !== p.key));
      }}
    />
  );
  // The working row from the moment a message goes, its dots alone until the
  // agent says what it is doing: it is where the agent's answer will start,
  // and adding it a beat after the message is what made the chat jump.
  const sending = unsent.some((p) => !p.behind && statusOf(p) === 'sending');
  // The hand-over: a message reaches the conversation a moment before the
  // desktop says the agent is at work. The row stays across it, as it was —
  // gone with the sending ring and back with the agent's first word, it moved
  // the chat twice for one message. It goes once the agent works, answers, or
  // takes longer than it should.
  if (seen.yours !== lastYours || seen.sending !== sending) {
    setSeen({ yours: lastYours, sending });
    if (lastYours > seen.yours && seen.sending && !sending && !working) setHandedAt(now);
  }
  if (handedAt > 0 && working) setHandedAt(0);
  const replied = turns.slice(lastYours + 1).some((turn) => turn.kind === Turn_Kind.ASSISTANT);
  const handingOver = handedAt > 0 && !working && !replied && now - handedAt < HAND_OVER_MS;

  // The foot holds something — a message on its way, or the agent at work —
  // and is spaced off the turns above it as they are from each other. With
  // no turns, what says so above it brings its own room.
  const spaced = turns.length > 0 && (unsent.length > 0 || inQueue.length > 0 || ((working || sending || handingOver) && !compacting));
  return (
    <Screen>
      <View style={styles.fill}>
        <ChatBar
          title={background ? (backgroundSession?.name ?? sessionNames.get(keptKey) ?? 'Session') : (worktree?.name ?? 'Session')}
          picks={picks}
          connected={hostState?.status === 'connected'}
          busy={isBusy}
          onWorktree={worktree ? () => router.push(`/worktree/${hostId}/${encodeURIComponent(worktree.id)}`) : undefined}
          uncommitted={(worktree?.filesChanged ?? 0) > 0}
          onTerminal={openTerminal}
        />
        <ChatNote
          text={note}
          tone={notice.tone}
          toSettings={toSettings}
          onDismiss={() => setNote('')}
          onCopy={copy}
          jobs={runningJobs > 0 ? <ChatJobs count={runningJobs} onPress={() => setJobsOpen(true)} /> : undefined}
        />
        <ChatScrollContext.Provider value={scrollContext}>
          <FlashList
            ref={list}
            data={rows}
            keyExtractor={(row) => row.key}
            getItemType={(row) => (row.at === FOOT ? 'foot' : row.tools ? 'tools' : turns[row.at].kind)}
            // A conversation kept from before opens at its end: the foot is
            // the last row, past every turn. The list takes this only as it
            // is made, so one read after the chat opened is taken to its end
            // by following it (see onContentSizeChange).
            initialScrollIndex={rows.length - 1}
            drawDistance={DRAW_AHEAD}
            renderScrollComponent={ChatScroll}
            ItemSeparatorComponent={Gap}
            style={styles.fill}
            contentContainerStyle={[styles.page, missing && styles.pageFill]}
            // What the conversation is when it has no turns to show: why, or
            // that it is on its way. The Mac's lost one fills the page, to
            // say it in the middle.
            ListHeaderComponent={tooOld ? (
            <View style={styles.empty}>
              <Status
                text={`ket on ${hostState?.host.name ?? 'your Mac'} is older than this app. Update it — rebuild ket and start a new host — to read sessions here.`}
                onCopy={copy}
              />
              <Button title="Open the terminal" onPress={openTerminal} />
            </View>
          ) : missing ? (
            <View style={styles.lost}>
              <CantFind
                agent={agent ? agentLabel(agent) : 'The agent'}
                onRetry={() => lookNow.current()}
                onTerminal={openTerminal}
              />
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
          ) : startedAt > 0 && turns.length > 0 ? (
            <ChatStart at={startedAt} now={now} />
          ) : null}
            ListHeaderComponentStyle={missing ? styles.pageFill : undefined}
            renderItem={({ item }) =>
              item.at === FOOT ? (
                <View style={[styles.trail, spaced && styles.trailSpaced]}>
                {/* A message that starts the agent working goes before the
                    indicator: the desktop says the agent is working a moment
                    before its transcript has the message that set it off. */}
                {unsent.filter((p) => !p.behind).map(outgoing)}
                {(working || sending || handingOver) && !compacting ? <Working key={keptKey} agent={agent} what={working && worktree && !background ? detail(worktree) : ''} details /> : null}
                {inQueue.map(({ words, named, files }, index) => (
                  <Outgoing key={`q${index}`} text={words} files={files} named={files.length > 0 ? [] : named} status="queued" onCopy={copy} />
                ))}
                {unsent.filter((p) => p.behind).map(outgoing)}
                </View>
              ) : item.tools ? (
                <Tools turns={item.tools} onCopy={copy} />
              ) : (
                <TurnView
                  turn={turns[item.at]}
                  agent={agent}
                  carried={turns[item.at].kind === Turn_Kind.USER ? carriedFiles.get(carriedKey(keptKey, saidIn(turns[item.at]))) : undefined}
                  onCopy={copy}
                  delivered={item.at === lastYours && unsent.length === 0 && queued.length === 0}
                />
              )
            }
            // A tap in the conversation puts the keyboard away, and the draft
            // stays. With it up, a tap on a message or a button only does that.
            keyboardShouldPersistTaps="never"
            onScroll={(e) => {
              const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
              // What is under a card waiting on you is not in view.
              const hidden = room.get();
              const end = contentOffset.y + layoutMeasurement.height - hidden >= contentSize.height - 48;
              atEnd.current = end;
              if (end === away) setAway(!end);
              if (end && fresh) setFresh(false);
            }}
            // Every frame: the list draws what comes into view from these.
            scrollEventThrottle={16}
            // Shrunk by what grows under it — a taller draft, a note — while
            // the newest turn was in view: kept in view. A card waiting on you
            // floats over it, kept clear by `room`.
            onLayout={() => {
              if (atEnd.current) toEnd(false);
            }}
            onContentSizeChange={() => {
              if (atEnd.current) toEnd(false);
            }}
          />
        </ChatScrollContext.Provider>

        <KeyboardLift offset={{ closed: 0, opened: lift }}>
        {/* The desk under the chips and the composer. At rest they sit below
            the conversation, but the keyboard lifts them over it, and a
            conversation scrolled behind them showed through, under a fade
            that then looked like a dark band across it. */}
        <View pointerEvents="none" style={styles.desk} />
        {/* Over the conversation's foot, riding with the composer: a card
            waiting on you, and above it the fade and the jump button. */}
        <View pointerEvents="box-none" style={styles.foot}>
          <FadingEdge shown={away} />
          {away ? (
            <View pointerEvents="box-none" style={styles.jumpDock}>
              <TouchableOpacity
                accessibilityRole="button"
                accessibilityLabel={fresh ? 'Jump to the newest — new messages' : 'Jump to the newest'}
                onPress={() => {
                  atEnd.current = true;
                  setAway(false);
                  setFresh(false);
                  toEnd(true);
                }}
                activeOpacity={PRESSED}
                style={styles.jump}
              >
                <ChevronDown size={18} color={t.text} weight={2.2} />
                {fresh ? <View style={styles.jumpDot} /> : null}
              </TouchableOpacity>
            </View>
          ) : null}

          {tuck.card ? <Tucked tuck={tuck}>{drawCard(tuck.card)}</Tucked> : null}
        </View>

        {hearing ? (
          <View accessibilityLiveRegion="polite" style={styles.hearing}>
            <View style={styles.hearingDot} />
            <Text style={styles.hearingText}>
              <Text style={{ color: t.text }}>Listening</Text>
              {hearing.onDevice ? ' · transcribed on this iPhone' : ' · transcribed by Apple'}
            </Text>
          </View>
        ) : compacting ? (
          <Compacting style={styles.compacting} />
        ) : null}
        {/* Chips send at once, so not while photos wait to go with the next
            message, or while the mic is writing one. Nor while the agent is
            at work, from Send on: they fold away, and the chat has their
            room. Under a menu they stay where they are: the menu floats
            over them. */}
        <Fold shown={!tooOld && !quiet && !stopped && !message && files.length === 0 && !hearing && !isBusy && !handingOver}>
          <ScrollView
            horizontal
            showsHorizontalScrollIndicator={false}
            keyboardShouldPersistTaps="handled"
            style={styles.replies}
            contentContainerStyle={styles.repliesInner}
          >
            {commands.length > 0 ? (
              <TouchableOpacity
                accessibilityRole="button"
                accessibilityLabel="Commands"
                accessibilityState={{ expanded: listing }}
                onPress={() => {
                  setAttaching(false);
                  setListing(!listing);
                }}
                activeOpacity={PRESSED}
                style={[styles.reply, styles.slash, listing && { backgroundColor: t.selection }]}
              >
                <Text style={styles.slashText}>/</Text>
              </TouchableOpacity>
            ) : null}
            {snippets.map((snippet, index) => (
              <TouchableOpacity
                key={`s${index}`}
                accessibilityRole="button"
                accessibilityLabel={`Send ${snippet.name}`}
                accessibilityHint="Hold to edit it first"
                onPress={() => void send(snippet.body)}
                onLongPress={() => {
                  write(snippet.body);
                  input.current?.focus();
                }}
                activeOpacity={PRESSED}
                style={[styles.reply, styles.snippet]}
              >
                <Bookmark size={13} color={t.brand} weight={2.2} />
                <Text style={styles.replyText} numberOfLines={1}>
                  {snippet.name}
                </Text>
              </TouchableOpacity>
            ))}
            {REPLIES.map((text) => (
              <TouchableOpacity
                key={text}
                accessibilityRole="button"
                accessibilityLabel={`Send ${text}`}
                onPress={() => void send(text)}
                activeOpacity={PRESSED}
                style={styles.reply}
              >
                <Text style={styles.replyText}>{text}</Text>
              </TouchableOpacity>
            ))}
          </ScrollView>
        </Fold>
        {stopped ? (
          <View style={{ paddingBottom: composing ? 0 : rest }}>
            <LimitBar
              limit={stopped}
              onOpen={limitSheet.reopen}
              use={canHand ? handTo : undefined}
              onUse={canHand && handTo ? () => limitSheet.handTo(handTo) : undefined}
            />
          </View>
        ) : null}
        {composing ? (
          <KeyCommands title="Send" onSend={() => void send()} style={[styles.composer, { paddingBottom: rest }]}>
            {/* The + and / menus: floats hung from the composer's top edge,
                over the chat and the chips, which stay put under them. */}
            <FloatMenu
              inline={plusMenu}
              open={attaching}
              onClose={() => setAttaching(false)}
              label="Attach"
              sections={attachSections({
                older: canAttach ? undefined : (hostState?.host.name ?? 'your Mac'),
                pasteable,
                onPick: (source) => void attach(source),
              })}
            />
            <FloatMenu
              inline={slashMenu(!tooOld && !background)}
              open={!attaching && menu.length > 0}
              onClose={() => setListing(false)}
              label="Commands"
              sections={[{ items: menu.map((command) => ({ ...commandRow(command), onPress: () => void pick(command) })) }]}
            />
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
                  placeholder={messagePlaceholder(agent)}
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
                    ? () => setNote('A background session stops in agent view on your Mac. Open the terminal to stop it.', 'info')
                    : stop
                  : () => void send()
              }
            />
          </KeyCommands>
        ) : null}
        </KeyboardLift>
        <SessionPickers picks={picks} agent={agent ? agentLabel(agent) : 'Agent'} busy={isBusy} />
        <JobsSheet
          open={jobsOpen}
          onClose={() => setJobsOpen(false)}
          agent={located?.terminal?.agent ?? ''}
          jobs={jobs}
          stop={(job) => (conn ? conn.stopJob(terminalId, job.pid, job.startedAtMs) : Promise.resolve('Not connected to your Mac.'))}
        />
        {stopped ? (
          <LimitSheet
            open={limitSheet.open}
            onClose={limitSheet.close}
            agent={stoppedAgent}
            limit={stopped}
            use={handTo}
            hand={hand}
            to={limitSheet.to}
            onTo={limitSheet.handTo}
          />
        ) : null}
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

const styles = StyleSheet.create({
  fill: { flex: 1 },
  page: { paddingHorizontal: size.gutter + 4, paddingTop: 4, paddingBottom: 18 },
  // Between the conversation's rows, and between what its foot holds.
  gap: { height: 12 },
  trail: { gap: 12 },
  trailSpaced: { paddingTop: 12 },
  empty: { gap: 12, paddingVertical: 24, alignItems: 'flex-start' },
  // With nothing to show, the chat's own height, for a mark in its middle.
  pageFill: { flexGrow: 1 },
  lost: { flex: 1, justifyContent: 'center', paddingVertical: 24 },
  // A cold session's card: a fact to weigh rather than something waiting on
  // you, so the float's own ground, not the attention tint.
  cold: {
    marginHorizontal: size.gutter,
    marginBottom: 8,
    padding: 12,
    gap: 8,
    borderRadius: size.radiusCard,
    ...surface.float,
  },
  coldTitle: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  // Dictating: a white dot and what is listening, where a note would be.
  hearing: { flexDirection: 'row', alignItems: 'center', gap: 7, paddingHorizontal: size.gutter + 4, paddingBottom: 8 },
  hearingDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: t.accent },
  hearingText: { fontFamily: font.prose, fontSize: size.label, lineHeight: 18, color: t.dim },
  compacting: { marginHorizontal: size.gutter + 4, marginBottom: 10 },
  replies: { flexGrow: 0 },
  repliesInner: { paddingHorizontal: size.gutter, paddingBottom: 8, gap: 6 },
  // A chip: a reply, a snippet, or `/`. Gone once there is a draft.
  reply: {
    height: 30,
    maxWidth: 220,
    paddingHorizontal: 11,
    borderRadius: 15,
    ...surface.float,
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
  field: { flex: 1, minWidth: 0, minHeight: size.control, borderRadius: size.control / 2, ...surface.well },
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
  // Past the composer's foot by more than the keyboard's rounded corners,
  // which would show the conversation beside them. Off the screen at rest.
  desk: { position: 'absolute', top: 0, left: 0, right: 0, bottom: -DESK_BELOW, backgroundColor: t.backdrop },
  // Hung from the composer's top edge, as tall as the card in it: what
  // floats over the conversation's foot hangs from its top.
  foot: { position: 'absolute', left: 0, right: 0, bottom: '100%' },
  jumpDock: { position: 'absolute', top: -54, right: size.gutter + 4 },
  jump: {
    width: 40,
    height: 40,
    borderRadius: 20,
    ...surface.float,
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
