// One live session per paired host, shared by every screen that shows it.
//
// It reconnects on its own — with the resume cursor, so a phone that dropped
// off for a moment picks up where it was — matches replies to requests, and
// renews a terminal's control lease while the phone holds it. Screens
// subscribe to what they draw and never touch the session directly.

import './polyfills';

import * as Device from 'expo-device';
import { AppState, type AppStateStatus, type NativeEventSubscription } from 'react-native';
import {
  type Answer_Choice,
  AnswerSchema,
  type Backlog,
  BacklogPriority,
  EditBacklog_Action,
  EditBacklogSchema,
  GetBacklogSchema,
  StartNoteSchema,
  type Changes,
  type Conversation,
  type Diff,
  type Device as PairedDevice,
  type Envelope,
  ErrorCode,
  GetChangesSchema,
  GetConversationSchema,
  GetDiffSchema,
  GetSnippetsSchema,
  ListDevicesSchema,
  MergeWorktreeSchema,
  RevokeDeviceSchema,
  StartWorkSchema,
  type HostSnapshot,
  InputSchema,
  Lease_Action,
  LeaseSchema,
  ResizeSchema,
  type Remote,
  Respond_PickSchema,
  RespondSchema,
  Signal_Kind,
  SignalSchema,
  SubscribeSchema,
  UnsubscribeSchema,
  create,
  envelope,
  resume,
  toBase64Url,
} from '@ket/remote';

import { phoneKey, saveHost, type SavedHost, toPaired } from './store';
import { versionLabel } from './version';

export type Status = 'connecting' | 'connected' | 'offline';

/** One change to a backlog note — see `HostConnection.editBacklog`. A save
 * with no priority leaves the note's as it was. */
export type BacklogChange =
  | { save: { title: string; body: string; priority?: BacklogPriority } }
  | { action: 'done' | 'reopen' | 'remove' };

/** An answer to a question or a plan — see `HostConnection.respond`. */
export type PromptAnswer = { picks?: { choices: number[]; other: string }[]; approve?: boolean; feedback?: string };

type Listener = (message: Envelope) => void;

/** How long a lease lasts on the host, and how often the phone renews it. */
const RENEW_MS = 20_000;

/** How long the desktop has to welcome a session once the handshake is done. */
const WELCOME_TIMEOUT_MS = 15_000;

/** Away longer than this — locked, or in another app — and the phone dials
 * the desktop again as it comes back rather than trusting the old socket.
 * iOS suspends an app in the background, and its socket seldom outlives more
 * than a moment of that, often without a close the phone ever hears. */
const RESUME_AFTER_MS = 5_000;

/** New work this phone asked for that the desktop has not shown yet: the
 * desktop answers as soon as it has the request, and makes the worktree —
 * fetching, checking out, starting the agent — after, which can take a while.
 * The list stands a row in for it meanwhile, so asking reads as done. */
export interface Starting {
  project: string;
  /** What the agent was asked to do, which the row is named after. */
  prompt: string;
  at: number;
  /** The project's worktrees when it was asked for: the first that is not
   * among them is the new one. */
  before: string[];
}

/** How long a row stands in for new work before it is given up on — the
 * desktop said it would start it, but something stopped it on the way. */
const STARTING_FOR_MS = 120_000;

export class HostConnection {
  readonly host: SavedHost;
  status: Status = 'connecting';
  snapshot: HostSnapshot | null = null;
  /** See [`Starting`]. */
  starting: Starting[] = [];
  error: string | null = null;
  /** The desktop's protocol minor, from its welcome — what it can answer.
   * 7 reads sessions as conversations and takes new work. */
  hostMinor = 0;
  /** When the phone last lost the desktop — or first tried to reach it —
   * while it has not got it back; `null` while connected. */
  downSince: number | null = Date.now();
  /** Attempts since the last time it connected. */
  attempts = 0;

  private remote: Remote | null = null;
  private nextRequest = 10n;
  /** Requests waiting for their answer: each fails at once if the session
   * ends, rather than waiting out its timeout on a socket that is gone. */
  private readonly pending = new Map<bigint, { resolve: (reply: Envelope) => void; reject: (error: Error) => void }>();
  private readonly listeners = new Set<Listener>();
  private readonly changed = new Set<() => void>();
  private readonly leases = new Map<bigint, ReturnType<typeof setInterval>>();
  private closed = false;
  private backoff = 1000;
  /** Ends the wait between attempts early — see [`retry`]. */
  private wake: (() => void) | null = null;
  /** When the app went to the background, while it is there. */
  private backgroundedAt: number | null = null;
  /** The session is being ended on purpose, so its end is not an error. */
  private redialing = false;
  private readonly appState: NativeEventSubscription;

  constructor(host: SavedHost) {
    this.host = host;
    this.appState = AppState.addEventListener('change', (next) => this.foreground(next));
    void this.run();
  }

  /** Coming back from the lock screen or another app: a fresh session if it
   * was away for long, so a screen asking the desktop gets an answer now
   * rather than after its request times out on a dead socket. */
  private foreground(next: AppStateStatus): void {
    if (next === 'background') {
      this.backgroundedAt ??= Date.now();
      return;
    }
    if (next !== 'active') return;
    const away = this.backgroundedAt === null ? 0 : Date.now() - this.backgroundedAt;
    this.backgroundedAt = null;
    if (away < RESUME_AFTER_MS || this.closed) return;
    this.backoff = 0;
    if (this.remote) {
      this.redialing = true;
      this.remote.close();
    }
    this.wake?.();
  }

  /** Called whenever status, snapshot or error change. */
  onChange(callback: () => void): () => void {
    this.changed.add(callback);
    return () => this.changed.delete(callback);
  }

  /** Every envelope from the host, for a screen that wants the stream. */
  onMessage(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const callback of this.changed) callback();
  }

  private async run(): Promise<void> {
    while (!this.closed) {
      try {
        await this.session();
        this.backoff = 1000;
      } catch (error) {
        if (!this.redialing) this.error = error instanceof Error ? error.message : String(error);
      }
      this.redialing = false;
      this.remote = null;
      this.clearLeases();
      for (const { reject } of this.pending.values()) reject(new Error('the connection to the desktop dropped'));
      this.pending.clear();
      if (this.closed) return;
      this.status = 'offline';
      this.downSince ??= Date.now();
      this.notify();
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, this.backoff);
        this.wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
      this.wake = null;
      this.backoff = Math.min(Math.max(this.backoff * 2, 1000), 30_000);
    }
  }

  /** Tries again now, rather than after the backoff: a person pressed Retry. */
  retry(): void {
    this.backoff = 1000;
    if (this.status !== 'connected') this.remote?.close();
    this.wake?.();
  }

  private async session(): Promise<void> {
    this.status = 'connecting';
    this.attempts += 1;
    this.notify();
    const keys = await phoneKey();
    const cursor = this.host.cursor;
    const remote = await resume(toPaired(this.host), keys, {
      deviceName: Device.deviceName ?? 'phone',
      appVersion: versionLabel,
      resumeEpoch: cursor ? BigInt(cursor.epoch) : 0n,
      resumeSeq: cursor ? BigInt(cursor.seq) : 0n,
    });
    this.remote = remote;
    let epoch = cursor ? BigInt(cursor.epoch) : 0n;
    let seq = cursor ? BigInt(cursor.seq) : 0n;
    let lastSaved = Date.now();

    for (;;) {
      // Until the welcome, a silent desktop is a failed attempt, not a wait.
      const message =
        this.status === 'connected'
          ? await remote.recv()
          : await remote.recv(WELCOME_TIMEOUT_MS).catch(() => {
              throw new Error('the desktop did not answer');
            });
      if (message.seq > seq) seq = message.seq;
      switch (message.payload.case) {
        case 'welcome':
          this.hostMinor = message.minor;
          if (message.payload.value.hostEpoch !== epoch) seq = message.seq;
          epoch = message.payload.value.hostEpoch;
          this.status = 'connected';
          this.error = null;
          this.downSince = null;
          this.attempts = 0;
          this.notify();
          break;
        case 'snapshot':
          this.snapshot = message.payload.value;
          this.settleStarting();
          this.notify();
          break;
        case 'error':
          if (message.payload.value.code === ErrorCode.UNSUPPORTED_VERSION) {
            this.error = message.payload.value.message;
            this.notify();
          }
          break;
      }
      const waiter = message.requestId !== 0n ? this.pending.get(message.requestId) : undefined;
      if (waiter) {
        this.pending.delete(message.requestId);
        waiter.resolve(message);
      }
      for (const listener of this.listeners) listener(message);

      // The cursor, saved now and then rather than on every envelope.
      if (Date.now() - lastSaved > 2000) {
        lastSaved = Date.now();
        this.host.cursor = { epoch: epoch.toString(), seq: seq.toString() };
        void saveHost(this.host);
      }
    }
  }

  private send(payload: Envelope['payload'], idempotent = false): Promise<Envelope> {
    const remote = this.remote;
    if (!remote) return Promise.reject(new Error('not connected'));
    const id = this.nextRequest++;
    const message = envelope(id, payload);
    if (idempotent) {
      const nonce = new Uint8Array(12);
      crypto.getRandomValues(nonce);
      message.idempotencyKey = toBase64Url(nonce);
    }
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      try {
        remote.send(message);
      } catch (error) {
        this.pending.delete(id);
        reject(error);
      }
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error('no answer from the host'));
      }, 10_000);
    });
  }

  /** The terminals a screen is showing, so a refused subscribe is asked
   * again only while one still is. */
  private watched = new Set<bigint>();

  /** Asks for a terminal's picture and output. `onRefused` hears why, each
   * time the desktop says no, so the screen can say so rather than sit empty. */
  subscribe(terminalId: bigint, onRefused?: (why: string) => void, attempt = 0): void {
    if (attempt === 0) this.watched.add(terminalId);
    void this.send({ case: 'subscribe', value: create(SubscribeSchema, { terminalId }) })
      .then((reply) => {
        if (reply.payload.case !== 'error') return;
        onRefused?.(reply.payload.value.message);
        // The desktop limits how fast a phone asks for costly things. A
        // refused subscribe left the terminal blank for good; it asks again,
        // a little later each time up to every 5 s, for as long as the
        // terminal is on screen — which is what its "Still trying" says.
        const refused = reply.payload.value.code === ErrorCode.BACKPRESSURE;
        if (refused && this.watched.has(terminalId)) {
          setTimeout(() => {
            if (this.watched.has(terminalId)) this.subscribe(terminalId, onRefused, attempt + 1);
          }, Math.min(1000 * (attempt + 1), 5000));
        }
      })
      .catch(() => {});
  }

  unsubscribe(terminalId: bigint): void {
    this.watched.delete(terminalId);
    void this.send({ case: 'unsubscribe', value: create(UnsubscribeSchema, { terminalId }) }).catch(
      () => {},
    );
  }

  /** Takes control of a terminal's input, and keeps it until released or
   * taken back at the desktop. `false` if another device has it. */
  async takeControl(terminalId: bigint): Promise<boolean> {
    const reply = await this.send(
      { case: 'lease', value: create(LeaseSchema, { terminalId, action: Lease_Action.ACQUIRE }) },
      true,
    );
    if (reply.payload.case !== 'ack') return false;
    const renew = setInterval(() => {
      this.send(
        { case: 'lease', value: create(LeaseSchema, { terminalId, action: Lease_Action.RENEW }) },
        true,
      ).catch(() => {});
    }, RENEW_MS);
    this.leases.get(terminalId) && clearInterval(this.leases.get(terminalId));
    this.leases.set(terminalId, renew);
    return true;
  }

  releaseControl(terminalId: bigint): void {
    const renew = this.leases.get(terminalId);
    if (renew) clearInterval(renew);
    this.leases.delete(terminalId);
    void this.send(
      { case: 'lease', value: create(LeaseSchema, { terminalId, action: Lease_Action.RELEASE }) },
      true,
    ).catch(() => {});
  }

  private clearLeases(): void {
    for (const renew of this.leases.values()) clearInterval(renew);
    this.leases.clear();
  }

  /** Types into a terminal. Resolves `false` if the phone no longer has
   * control — the desktop took it back — and stops renewing then. */
  async type(terminalId: bigint, text: string): Promise<boolean> {
    const reply = await this.send(
      {
        case: 'input',
        value: create(InputSchema, { terminalId, bytes: new TextEncoder().encode(text) }),
      },
      true,
    );
    if (reply.payload.case === 'error' && reply.payload.value.code === ErrorCode.NOT_CONTROLLER) {
      const renew = this.leases.get(terminalId);
      if (renew) clearInterval(renew);
      this.leases.delete(terminalId);
      return false;
    }
    return true;
  }

  /** The phones paired with this desktop, this one marked. */
  async devices(): Promise<PairedDevice[]> {
    const reply = await this.send({ case: 'listDevices', value: create(ListDevicesSchema, {}) });
    if (reply.payload.case !== 'devices') throw new Error('the desktop did not list its devices');
    return reply.payload.value.devices;
  }

  /** New work in a project: ket on the desktop makes a worktree for it and
   * starts the project's agent on `prompt`. Resolves `null` once ket has it,
   * or the desktop's reason it could not — most often that ket isn't open. */
  async startWork(projectId: string, prompt: string, agent = ''): Promise<string | null> {
    const before =
      this.snapshot?.projects.find((p) => p.id === projectId)?.worktrees.map((w) => w.id) ?? [];
    const reply = await this.send(
      { case: 'startWork', value: create(StartWorkSchema, { projectId, prompt, agent }) },
      true,
    );
    if (reply.payload.case === 'error') return reply.payload.value.message;
    this.starting = [...this.starting, { project: projectId, prompt, at: Date.now(), before }];
    setTimeout(() => {
      this.settleStarting();
      this.notify();
    }, STARTING_FOR_MS + 50);
    this.notify();
    return null;
  }

  /** Lets go of the rows standing in for work that has arrived, or that
   * has taken too long to. */
  private settleStarting(): void {
    if (this.starting.length === 0) return;
    const now = Date.now();
    this.starting = this.starting.filter((work) => {
      if (now - work.at > STARTING_FOR_MS) return false;
      const project = this.snapshot?.projects.find((p) => p.id === work.project);
      return !project?.worktrees.some((w) => !work.before.includes(w.id));
    });
  }

  /** A project's backlog, open notes and done (desktop 1.9). */
  async backlog(projectId: string): Promise<Backlog> {
    const reply = await this.send({ case: 'getBacklog', value: create(GetBacklogSchema, { projectId }) });
    if (reply.payload.case === 'backlog') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not send the backlog');
  }

  /** Writes a note — a new one at the top when `noteId` is empty — marks it
   * done, reopens it or deletes it. Resolves the backlog as it then is. */
  async editBacklog(projectId: string, noteId: string, change: BacklogChange): Promise<Backlog> {
    const action =
      'save' in change
        ? EditBacklog_Action.SAVE
        : { done: EditBacklog_Action.DONE, reopen: EditBacklog_Action.REOPEN, remove: EditBacklog_Action.REMOVE }[change.action];
    const text =
      'save' in change
        ? { title: change.save.title, body: change.save.body, priority: change.save.priority ?? BacklogPriority.UNSPECIFIED }
        : { title: '', body: '' };
    const reply = await this.send(
      { case: 'editBacklog', value: create(EditBacklogSchema, { projectId, noteId, action, ...text }) },
      true,
    );
    if (reply.payload.case === 'backlog') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not save it');
  }

  /** Starts a note as work, as the desktop's backlog does. `null` once ket
   * has it — the note is marked done when its worktree exists — or why not. */
  async startNote(projectId: string, noteId: string): Promise<string | null> {
    const reply = await this.send({ case: 'startNote', value: create(StartNoteSchema, { projectId, noteId }) }, true);
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /** The files a worktree would bring to its base if merged now. */
  async changes(worktreeId: string): Promise<Changes> {
    const reply = await this.send({ case: 'getChanges', value: create(GetChangesSchema, { worktreeId }) });
    if (reply.payload.case === 'changes') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not list the changes');
  }

  /** One changed file's diff. */
  async diff(worktreeId: string, path: string): Promise<Diff> {
    const reply = await this.send({ case: 'getDiff', value: create(GetDiffSchema, { worktreeId, path }) });
    if (reply.payload.case === 'diff') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not send the diff');
  }

  /** Merges a worktree into its base with ket's own merge. `null` once ket
   * has it — what it did shows in the next snapshots — or why it could not. */
  async merge(worktreeId: string): Promise<string | null> {
    const reply = await this.send(
      { case: 'mergeWorktree', value: create(MergeWorktreeSchema, { worktreeId }) },
      true,
    );
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /** The snippets saved in ket on the desktop. */
  async snippets(): Promise<{ name: string; body: string }[]> {
    const reply = await this.send({ case: 'getSnippets', value: create(GetSnippetsSchema, {}) });
    return reply.payload.case === 'snippets' ? reply.payload.value.snippets : [];
  }

  /** What the agent in a terminal has said since `after` — 0 for the recent
   * end of its session. Ask again with the reply's `next` for what is new. */
  async conversation(terminalId: bigint, after: bigint): Promise<Conversation> {
    const reply = await this.send({
      case: 'getConversation',
      value: create(GetConversationSchema, { terminalId, after }),
    });
    if (reply.payload.case === 'conversation') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not send the conversation');
  }

  /** Stops the desktop accepting a phone; revoking this one forgets the desktop. */
  async revoke(id: string): Promise<void> {
    const reply = await this.send(
      { case: 'revokeDevice', value: create(RevokeDeviceSchema, { id }) },
      true,
    );
    if (reply.payload.case === 'error') throw new Error(reply.payload.value.message);
  }

  /**
   * Answers one permission prompt — the `Question` the phone showed, by its
   * id and the terminal it is in; the desktop presses the agent's own key
   * only if that terminal is still showing that very prompt. Needs no
   * control. Resolves `null` once pressed, or the desktop's reason it would
   * not — most often that the prompt was answered or replaced meanwhile.
   */
  async answer(question: { id: bigint; terminalId: bigint }, choice: Answer_Choice): Promise<string | null> {
    const reply = await this.send(
      {
        case: 'answer',
        value: create(AnswerSchema, { terminalId: question.terminalId, questionId: question.id, choice }),
      },
      true,
    );
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /**
   * Answers a question the agent asked with options, or a plan it wants to
   * go ahead with — a `Question` with `asks` or `plan` (desktop 1.8). For a
   * question, one pick per question in order: the indexes chosen, or what was
   * written instead. For a plan, approve, or keep planning with feedback.
   * The desktop hands it to the agent only if that terminal is still showing
   * that very prompt. Resolves `null` once it has, or the desktop's reason.
   */
  async respond(question: { id: bigint; terminalId: bigint }, answer: PromptAnswer): Promise<string | null> {
    const reply = await this.send(
      {
        case: 'respond',
        value: create(RespondSchema, {
          terminalId: question.terminalId,
          questionId: question.id,
          picks: (answer.picks ?? []).map((pick) => create(Respond_PickSchema, pick)),
          approve: answer.approve ?? false,
          feedback: answer.feedback ?? '',
        }),
      },
      true,
    );
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /** Fits a terminal to the phone's screen, control or not; the desktop's
   * size comes back when the phone unsubscribes or goes. `false` while
   * another phone has control. */
  async resize(terminalId: bigint, cols: number, rows: number): Promise<boolean> {
    const reply = await this.send({ case: 'resize', value: create(ResizeSchema, { terminalId, cols, rows }) }, true);
    return reply.payload.case !== 'error';
  }

  /** Interrupts what runs in a terminal. Needs no control. */
  interrupt(terminalId: bigint): void {
    void this.send(
      { case: 'signal', value: create(SignalSchema, { terminalId, kind: Signal_Kind.INTERRUPT }) },
      true,
    ).catch(() => {});
  }

  close(): void {
    this.closed = true;
    this.appState.remove();
    this.clearLeases();
    this.remote?.close();
  }
}

const connections = new Map<string, HostConnection>();

/** Closes and drops the connection to a host being forgotten. */
export function dropConnection(id: string): void {
  connections.get(id)?.close();
  connections.delete(id);
}

/** The one connection to a host, made on first use. */
export function connection(host: SavedHost): HostConnection {
  let existing = connections.get(host.id);
  if (!existing) {
    existing = new HostConnection(host);
    connections.set(host.id, existing);
  }
  return existing;
}
