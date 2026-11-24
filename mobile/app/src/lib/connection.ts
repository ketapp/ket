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
  Activity,
  BackgroundSession_State,
  type Answer_Choice,
  AnswerSchema,
  AutomationEdit,
  type AutomationSummary,
  EditAutomationSchema,
  GetAutomationsSchema,
  type Backlog,
  BacklogPriority,
  DialError,
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
  FixChecksSchema,
  AddressCommentsSchema,
  GetPullRequestDraftSchema,
  OpenPullRequestSchema,
  MergePullRequestSchema,
  UpdatePullRequestSchema,
  ReadyPullRequestSchema,
  ReopenPullRequestSchema,
  PinWorktreeSchema,
  RenameWorktreeSchema,
  RegisterPushSchema,
  UnregisterPushSchema,
  type PushEnvironment,
  type PushKind,
  DeleteWorktreeSchema,
  WakeTerminalSchema,
  type PullRequestDraft,
  type PullRequestOpened,
  type PullRequestMergeMethod,
  GetChangesSchema,
  GetConversationSchema,
  OpenAgentSessionSchema,
  SetModeSchema,
  SetModelSchema,
  GetDiffSchema,
  GetSnippetsSchema,
  AttachmentSchema,
  SendFilesSchema,
  ListDevicesSchema,
  MergeWorktreeSchema,
  RevokeDeviceSchema,
  ReviewCommentSchema,
  SendReviewSchema,
  StartWorkSchema,
  GetStartOptionsSchema,
  type StartOptions,
  StartSessionSchema,
  type HostSnapshot,
  InputSchema,
  Lease_Action,
  LeaseSchema,
  ResizeSchema,
  type Remote,
  Respond_PickSchema,
  RespondSchema,
  Signal_Kind,
  MarkSeenSchema,
  MarkUnreadSchema,
  SignalSchema,
  StartFreshSchema,
  SubscribeSchema,
  UnsubscribeSchema,
  create,
  envelope,
  resume,
  toBase64Url,
  validRelayUrl,
} from '@ket/remote';

import { phoneKey, saveHost, type SavedHost, toPaired } from './store';
import { addManualAddress, dialAddresses, removeManualAddress } from './addresses';
import { versionLabel } from './version';
import { bindTasks, worktreeOf } from './starting';

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

/** How long it has to answer a message with files: the upload, then a few
 * seconds while the agent takes its images in. */
const FILES_ANSWER_MS = 60_000;

/** How long the desktop has to welcome a session once the handshake is done. */
const WELCOME_TIMEOUT_MS = 15_000;

/** How long a FixChecks waits for its answer, across reconnects: the
 * desktop gives reading the failed logs up to a minute (two `gh` calls a job,
 * side by side, 30 s each) and then the agent another minute to take the
 * message before saying it is still starting. */
const FIX_CHECKS_WAIT_MS = 150_000;

/** How long a review waits for its answer, across reconnects: the desktop
 * gives a ket window ten seconds to take it, and says it did not go after. */
const REVIEW_WAIT_MS = 20_000;

/** How long opening a pull request waits for its answer, across reconnects:
 * a commit that runs the repository's hooks, a push, and two `gh` calls. */
const PULL_REQUEST_WAIT_MS = 180_000;

/** How long a delete is waited for: the desktop gives its window two
 * minutes to walk the checkout and ask git, and this is a little longer. */
const DELETE_WAIT_MS = 150_000;
/** How long a wake waits: the desktop resumes the agent, which reads its
 * session back first. The desktop gives up before this. */
const WAKE_WAIT_MS = 60_000;

/** How opening a pull request went: opened, or pushed to one already open —
 * or why not. */
export type OpenedPullRequest = { opened: PullRequestOpened } | { why: string };

/** One comment of a review, as `sendReview` sends it. */
export type ReviewNote = { path: string; lines: string; text: string; comment: string };

/** What a request still waiting is failed with when its session ends. */
const DROPPED = 'the connection to the desktop dropped';

/** How a Fix checks with agent went: given to the agent — to one still
 * starting, when `starting` — or why it was not. */
export type FixedChecks = { starting: boolean } | { why: string };

/** How Address comments with agent went, the same shape as Fix checks. */
export type AddressedComments = { starting: boolean } | { why: string };

/** A fresh idempotency key, naming one change however often it is sent. */
function newKey(): string {
  const nonce = new Uint8Array(12);
  crypto.getRandomValues(nonce);
  return toBase64Url(nonce);
}

/** Away longer than this — locked, or in another app — and the phone checks
 * the session still works as it comes back, rather than trusting it: iOS
 * suspends an app in the background, and its socket can die meanwhile
 * without a close the phone ever hears. See `probe`. */
const RESUME_AFTER_MS = 5_000;

/** How long a session back from the background has to show it still works
 * — any frame from the desktop, the answer to a cheap request among them —
 * before the phone drops it and dials again. Mostly it does: the host's log
 * showed the old socket still open on nearly every return, so dialling
 * again each time threw away a working session for a second or more of
 * handshake and snapshot. */
const PROBE_MS = 1_500;

/** How often snapshots are drawn while they come faster than the desktop
 * makes them. A session kept through the lock screen is handed, at once,
 * every snapshot the desktop sent while the app was suspended — the relay
 * holds up to 8 MB of them — and drawing each in turn kept the app too busy
 * to take a tap until the last was in. The desktop makes one a second at
 * most, so a live one is never held back. */
const SNAPSHOT_DRAW_MS = 250;

/** New work this phone asked for that the desktop has not shown yet: the
 * desktop answers as soon as it has the request, and makes the worktree —
 * fetching, checking out, starting the agent — after, which can take a while.
 * The list stands a row in for it meanwhile, so asking reads as done. */
export interface Starting {
  project: string;
  /** What the agent was asked to do, which the row is named after. */
  prompt: string;
  /** The agent asked for, or empty for the project's own. */
  agent: string;
  at: number;
  /** The project's worktrees when it was asked for: the new one is not
   * among them, and is on the branch the prompt names — see
   * `worktreeOf` in lib/starting. */
  before: string[];
  /** The base, model and effort picked for it, when any were: what asking
   * again asks for. */
  choices?: StartChoices;
  /** Bound once, so removing a tab cannot move the prompt to another one. */
  terminal?: bigint;
  /** When that terminal first appeared; delivery does not time worktree creation. */
  openedAt?: number;
  /** The first conversation read in that terminal. */
  session?: string;
  /** A settled prompt is retained as history, but no longer restored as unsent. */
  delivery?: 'delivered' | 'dismissed' | 'superseded';
}

/** A phone's picks for new work, each empty for the desktop's default. */
export interface StartChoices {
  base?: string;
  model?: string;
  effort?: string;
}

/** How many requests [`HostConnection.asked`] keeps. */
const ASKED_KEPT = 20;

/** How long a row stands in for new work before it is given up on — the
 * desktop said it would start it, but something stopped it on the way. */
export const STARTING_FOR_MS = 120_000;

/** The desktop turned a request away for now: it limits how fast a phone
 * asks for costly things. Not a failure; asking again shortly is. */
export class DesktopBusy extends Error {}

/** What a connection is at one moment, for a screen to draw from: made anew
 * whenever any of it changes and never edited, so React, and the React
 * Compiler, which reuses what it worked out from a value it has seen before,
 * can tell one moment from the next. Read it with `useHostState` (lib/hosts.ts);
 * outside React, from `HostConnection.state`. What a connection does stays
 * on the connection. */
export interface ConnectionState {
  /** The paired desktop as saved, but for the cursor it resumes from. */
  readonly host: Readonly<Omit<SavedHost, 'cursor'>>;
  readonly status: Status;
  readonly snapshot: HostSnapshot | null;
  /** See [`Starting`]: new work asked for whose worktree is not listed yet. */
  readonly starting: readonly Starting[];
  /** Every request this connection made, oldest first, the last few: for the
   * chat that follows one (see app/starting), which may open after the
   * worktree is listed and the row for it is gone. */
  readonly asked: readonly Starting[];
  readonly error: string | null;
  /** The desktop's protocol minor, from its welcome — what it can answer.
   * 7 reads sessions as conversations and takes new work. */
  readonly hostMinor: number;
  /** The address of the open session, or null between sessions. */
  readonly activeAddress: string | null;
  /** The live address, or the last one that worked while reconnecting. */
  readonly displayAddress: string | null;
  /** When the phone last lost the desktop — or first tried to reach it —
   * while it has not got it back; `null` while connected. */
  readonly downSince: number | null;
  /** Attempts since the last time it connected. */
  readonly attempts: number;
  /** When the open session was welcomed; `null` between sessions. */
  readonly upSince: number | null;
  /** What each saved address said the last time none of them answered,
   * by address; empty while connected or after a failure past the dial. */
  readonly failures: Readonly<Record<string, string>>;
  /** When the phone first heard from the desktop as it is now: this
   * connection's first welcome, or the first after the desktop restarted.
   * Its list can still be filling for a moment after — see lib/settling. */
  readonly freshSince: number | null;
  /** When the snapshot came; `null` before the first. */
  readonly snapshotAt: number | null;
}

/** Whether `kept`, a connection state's copy of a saved host, still matches
 * `host` field for field, the cursor aside. */
function sameHost(kept: ConnectionState['host'], host: SavedHost): boolean {
  const keys = (Object.keys(host) as (keyof SavedHost)[]).filter((key) => key !== 'cursor');
  return keys.length === Object.keys(kept).length && keys.every((key) => (kept as SavedHost)[key] === host[key]);
}

export class HostConnection {
  /** The paired desktop's id, which never changes. */
  readonly id: string;
  // What `state` is made from: changed in place as the connection goes, so
  // read by screens only through `state`.
  private readonly host: SavedHost;
  private status: Status = 'connecting';
  private snapshot: HostSnapshot | null = null;
  private starting: Starting[] = [];
  private asked: Starting[] = [];
  private error: string | null = null;
  private hostMinor = 0;
  private activeAddress: string | null = null;
  private downSince: number | null = Date.now();
  private attempts = 0;
  private upSince: number | null = null;
  private failures: Record<string, string> = {};
  private freshSince: number | null = null;
  private snapshotAt: number | null = null;
  /** The last `state` made, handed out again until something in it changes. */
  private made: ConnectionState | null = null;

  private remote: Remote | null = null;
  /** The epoch of the desktop the last session was welcomed by, to tell
   * when it restarted. */
  private hostEpoch: bigint | null = null;
  private nextRequest = 10n;
  /** Requests waiting for their answer: each fails at once if the session
   * ends, rather than waiting out its timeout on a socket that is gone. */
  private readonly pending = new Map<bigint, { resolve: (reply: Envelope) => void; reject: (error: Error) => void }>();
  private readonly listeners = new Set<Listener>();
  private readonly changed = new Set<() => void>();
  /** Prompts the desktop accepted or says are gone, held until a snapshot
   * stops listing them. A delayed snapshot must not bring a card back. */
  private readonly settledQuestions = new Set<string>();
  private readonly leases = new Map<bigint, ReturnType<typeof setInterval>>();
  private closed = false;
  private backoff = 1000;
  /** Ends the wait between attempts early — see [`retry`]. */
  private wake: (() => void) | null = null;
  /** When the app went to the background, while it is there. */
  private backgroundedAt: number | null = null;
  /** Frames had from the desktop, over every session: a count going up is
   * a session that works — see `probe`. */
  private heard = 0;
  /** When a snapshot was last drawn, and the draw waiting for the latest
   * one, if any — see `drawSnapshot`. */
  private snapshotDrawnAt = 0;
  private snapshotDraw: ReturnType<typeof setTimeout> | null = null;
  /** The session is being ended on purpose, so its end is not an error. */
  private redialing = false;
  private readonly appState: NativeEventSubscription;

  constructor(host: SavedHost) {
    this.id = host.id;
    this.host = host;
    this.appState = AppState.addEventListener('change', (next) => this.foreground(next));
    void this.run();
  }

  /** Coming back from the lock screen or another app, after a while away: a
   * session still up is kept if it shows it works (see `probe`), so a screen
   * asking the desktop gets its answer on it at once; one still dialling is
   * dialled again, and one down is dialled now rather than after its
   * backoff. */
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
    if (this.remote && this.status === 'connected') this.probe(this.remote);
    else if (this.remote) this.redial(this.remote);
    this.wake?.();
  }

  /** Keeps `remote` if anything comes from the desktop within PROBE_MS —
   * the answer to the snippets, the cheapest thing to ask, or whatever it
   * sent meanwhile — and otherwise drops it and dials again. */
  private probe(remote: Remote): void {
    const heard = this.heard;
    let settled = false;
    const settle = (works: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (!works && this.remote === remote && !this.closed) this.redial(remote);
    };
    const timer = setTimeout(() => settle(this.heard > heard), PROBE_MS);
    this.request({ case: 'getSnippets', value: create(GetSnippetsSchema, {}) }, '', PROBE_MS).then(
      () => settle(true),
      () => settle(this.heard > heard),
    );
  }

  /** Ends `remote` to dial again at once, which is not a failure to show. */
  private redial(remote: Remote): void {
    this.redialing = true;
    remote.close();
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

  /** `notify` for a new snapshot, at most once every SNAPSHOT_DRAW_MS: in a
   * run of them only the latest is drawn, and the ones before it, already
   * out of date, never are. */
  private drawSnapshot(): void {
    if (this.snapshotDraw) return;
    const wait = this.snapshotDrawnAt + SNAPSHOT_DRAW_MS - Date.now();
    const draw = () => {
      this.snapshotDraw = null;
      this.snapshotDrawnAt = Date.now();
      this.notify();
    };
    if (wait <= 0) draw();
    else this.snapshotDraw = setTimeout(draw, wait);
  }

  /** What this connection is now — see `ConnectionState`. The same object
   * until something in it changes, then a new one. */
  get state(): ConnectionState {
    const was = this.made;
    if (
      was &&
      was.status === this.status &&
      was.snapshot === this.snapshot &&
      was.starting === this.starting &&
      was.asked === this.asked &&
      was.error === this.error &&
      was.hostMinor === this.hostMinor &&
      was.activeAddress === this.activeAddress &&
      was.downSince === this.downSince &&
      was.attempts === this.attempts &&
      was.upSince === this.upSince &&
      was.failures === this.failures &&
      was.freshSince === this.freshSince &&
      was.snapshotAt === this.snapshotAt &&
      sameHost(was.host, this.host)
    ) {
      return was;
    }
    const { cursor: _cursor, ...host } = this.host;
    this.made = {
      host,
      status: this.status,
      snapshot: this.snapshot,
      starting: this.starting,
      asked: this.asked,
      error: this.error,
      hostMinor: this.hostMinor,
      activeAddress: this.activeAddress,
      displayAddress: this.addressForDisplay(),
      downSince: this.downSince,
      attempts: this.attempts,
      upSince: this.upSince,
      failures: this.failures,
      freshSince: this.freshSince,
      snapshotAt: this.snapshotAt,
    };
    return this.made;
  }

  /** The live address, or the last one that worked while reconnecting. */
  private addressForDisplay(): string | null {
    const addresses = dialAddresses(this.host);
    return this.activeAddress ?? this.host.lastAddress ?? (addresses.length === 1 ? addresses[0] : null);
  }

  /** Addresses added by this phone stay separate from the desktop's list. */
  async addAddress(address: string): Promise<void> {
    Object.assign(this.host, addManualAddress(this.host, address));
    await saveHost(this.host);
    this.notify();
    if (this.status !== 'connected') this.retry();
  }

  async removeAddress(address: string): Promise<void> {
    Object.assign(this.host, removeManualAddress(this.host, address));
    if (this.host.lastAddress === address && !dialAddresses(this.host).includes(address)) {
      delete this.host.lastAddress;
    }
    await saveHost(this.host);
    this.notify();
  }

  private async run(): Promise<void> {
    while (!this.closed) {
      try {
        await this.session();
        this.backoff = 1000;
      } catch (error) {
        if (!this.redialing) {
          this.error = error instanceof Error ? error.message : String(error);
          this.failures = error instanceof DialError ? error.failures : {};
        }
      }
      this.redialing = false;
      this.remote = null;
      this.activeAddress = null;
      this.upSince = null;
      this.clearLeases();
      for (const { reject } of this.pending.values()) reject(new Error(DROPPED));
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
    this.activeAddress = remote.address ?? null;
    if (this.activeAddress && this.host.lastAddress !== this.activeAddress) {
      this.host.lastAddress = this.activeAddress;
      void saveHost(this.host);
    }
    this.notify();
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
      this.heard += 1;
      if (message.seq > seq) seq = message.seq;
      switch (message.payload.case) {
        case 'welcome':
          this.hostMinor = message.minor;
          if (message.payload.value.hostEpoch !== epoch) seq = message.seq;
          epoch = message.payload.value.hostEpoch;
          if (this.hostEpoch !== epoch) {
            this.freshSince = Date.now();
            this.settledQuestions.clear();
          }
          this.hostEpoch = epoch;
          this.status = 'connected';
          this.error = null;
          this.failures = {};
          this.downSince = null;
          this.upSince = Date.now();
          this.attempts = 0;
          this.notify();
          break;
        case 'snapshot':
          if (this.hostMinor >= 17) {
            const addresses = (message.payload.value.addresses ?? []).filter(validRelayUrl);
            if (JSON.stringify(this.host.addresses) !== JSON.stringify(addresses)) {
              this.host.addresses = addresses;
              if (this.host.lastAddress && !dialAddresses(this.host).includes(this.host.lastAddress)) {
                delete this.host.lastAddress;
              }
              void saveHost(this.host);
            }
          }
          this.snapshot = this.withoutSettledQuestions(message.payload.value, true);
          this.snapshotAt = Date.now();
          this.settleStarting();
          this.drawSnapshot();
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
    return this.request(payload, idempotent ? newKey() : '', 10_000);
  }

  /** Sends `payload` under idempotency key `key` — none, if empty — and
   * waits up to `waitMs` for its answer. */
  private request(payload: Envelope['payload'], key: string, waitMs: number): Promise<Envelope> {
    const remote = this.remote;
    if (!remote) return Promise.reject(new Error('not connected'));
    const id = this.nextRequest++;
    const message = envelope(id, payload);
    if (key) message.idempotencyKey = key;
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
      }, waitMs);
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

  /**
   * A message with photos and files for the agent in a terminal (desktop
   * 1.12): the desktop keeps the files, pastes the images into the agent's
   * input, and types the words and Enter after them, as `type` would. Needs
   * control, as `type` does: resolves `false` if the phone no longer has it,
   * and throws the desktop's reason when it refuses.
   */
  async sendFiles(terminalId: bigint, text: string, files: { name: string; bytes: Uint8Array }[]): Promise<boolean> {
    const reply = await this.request(
      {
        case: 'sendFiles',
        value: create(SendFilesSchema, {
          terminalId,
          text,
          attachments: files.map((file) => create(AttachmentSchema, { name: file.name, data: file.bytes })),
        }),
      },
      newKey(),
      FILES_ANSWER_MS,
    );
    if (reply.payload.case !== 'error') return true;
    if (reply.payload.value.code === ErrorCode.NOT_CONTROLLER) {
      const renew = this.leases.get(terminalId);
      if (renew) clearInterval(renew);
      this.leases.delete(terminalId);
      return false;
    }
    throw new Error(reply.payload.value.message);
  }

  /** Since 1.24: puts the agent in terminal `terminalId` in mode `mode`, one
   * of its `modes.choices`. The desktop presses the agent's own key until
   * it reports that mode, so this takes a moment. `false` when the phone
   * turns out not to have control. */
  async setMode(terminalId: bigint, mode: string): Promise<boolean> {
    const reply = await this.request({ case: 'setMode', value: create(SetModeSchema, { terminalId, mode }) }, newKey(), FILES_ANSWER_MS);
    if (reply.payload.case !== 'error') return true;
    if (reply.payload.value.code === ErrorCode.NOT_CONTROLLER) {
      const renew = this.leases.get(terminalId);
      if (renew) clearInterval(renew);
      this.leases.delete(terminalId);
      return false;
    }
    throw new Error(reply.payload.value.message);
  }

  /** Since 1.26: switches the agent in terminal `terminalId` to `model` and
   * `effort` — either empty to leave it — by its own commands, each waited
   * on until it confirms. `false` when the phone turns out not to have
   * control; a refusal throws with the desktop's reason. */
  async setModel(terminalId: bigint, model: string, effort: string): Promise<boolean> {
    const reply = await this.request(
      { case: 'setModel', value: create(SetModelSchema, { terminalId, model, effort }) },
      newKey(),
      FILES_ANSWER_MS,
    );
    if (reply.payload.case !== 'error') return true;
    if (reply.payload.value.code === ErrorCode.NOT_CONTROLLER) {
      const renew = this.leases.get(terminalId);
      if (renew) clearInterval(renew);
      this.leases.delete(terminalId);
      return false;
    }
    throw new Error(reply.payload.value.message);
  }

  /** Since 1.21: opens background session `session` in the agent view
   * terminal `terminalId` shows, then types `text` and Enter to it, if any.
   * `false` when the phone turns out not to have control. Agent view draws
   * each step, so this takes a second or two. */
  async openAgentSession(terminalId: bigint, session: string, text: string): Promise<boolean> {
    const reply = await this.request(
      { case: 'openAgentSession', value: create(OpenAgentSessionSchema, { terminalId, session, text }) },
      newKey(),
      FILES_ANSWER_MS,
    );
    if (reply.payload.case !== 'error') return true;
    if (reply.payload.value.code === ErrorCode.NOT_CONTROLLER) {
      const renew = this.leases.get(terminalId);
      if (renew) clearInterval(renew);
      this.leases.delete(terminalId);
      return false;
    }
    throw new Error(reply.payload.value.message);
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
  async startWork(projectId: string, prompt: string, agent = '', choices: StartChoices = {}): Promise<string | null> {
    const before =
      this.snapshot?.projects.find((p) => p.id === projectId)?.worktrees.map((w) => w.id) ?? [];
    const reply = await this.send(
      { case: 'startWork', value: create(StartWorkSchema, { projectId, prompt, agent, ...choices }) },
      true,
    );
    if (reply.payload.case === 'error') return reply.payload.value.message;
    const picked = Object.values(choices).some((value) => value);
    const work: Starting = { project: projectId, prompt, agent, at: Date.now(), before, ...(picked ? { choices } : {}) };
    this.starting = [...this.starting, work];
    this.asked = bindTasks([...this.asked, work].slice(-ASKED_KEPT), this.snapshot);
    setTimeout(() => {
      this.settleStarting();
      this.notify();
    }, STARTING_FOR_MS + 50);
    this.notify();
    return null;
  }

  /** Current desktop choices for a project's new work and sessions. */
  async startOptions(projectId: string): Promise<StartOptions> {
    const reply = await this.send({ case: 'getStartOptions', value: create(GetStartOptionsSchema, { projectId }) });
    if (reply.payload.case === 'startOptions') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not send start options');
  }

  /** Opens a fresh agent session or blank shell in an existing worktree. */
  async startSession(worktreeId: string, agent = '', prompt = '', model = '', effort = ''): Promise<string | null> {
    const reply = await this.send(
      { case: 'startSession', value: create(StartSessionSchema, { worktreeId, agent, prompt, model, effort }) }, true,
    );
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /** Lets go of the rows standing in for work that has arrived, or that
   * has taken too long to. */
  private settleStarting(): void {
    // A worktree can appear before its terminal, after its starting row has
    // already gone. Keep binding the retained requests on later snapshots.
    this.asked = bindTasks(this.asked, this.snapshot);
    if (this.starting.length === 0) return;
    const now = Date.now();
    this.starting = this.starting.filter((work) => {
      if (now - work.at > STARTING_FOR_MS) return false;
      const project = this.snapshot?.projects.find((p) => p.id === work.project);
      return worktreeOf(work, project) === undefined;
    });
  }

  /** Every automation in the projects this phone sees, those to come
   * soonest first (desktop 1.36). */
  async automations(): Promise<AutomationSummary[]> {
    const reply = await this.send({ case: 'getAutomations', value: create(GetAutomationsSchema, {}) });
    if (reply.payload.case === 'automations') return reply.payload.value.automations;
    throw new Error(
      reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not send its automations',
    );
  }

  /** Pauses, resumes, starts now or deletes an automation (desktop 1.36).
   * Resolves the list as it then is. */
  async editAutomation(automationId: string, edit: AutomationEdit): Promise<AutomationSummary[]> {
    const reply = await this.send(
      { case: 'editAutomation', value: create(EditAutomationSchema, { automationId, edit }) },
      true,
    );
    if (reply.payload.case === 'automations') return reply.payload.value.automations;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not change it');
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

  /** Hands a worktree's failed checks to its agent, as the desktop's Fix
   * checks with agent does (desktop 1.11), and says how it went.
   *
   * It takes a while, and the desktop carries on with it if the connection
   * drops meanwhile. So it is asked again from the next session under the
   * same key, and the desktop answers with how that went — waiting for it,
   * if need be — rather than giving the agent the message twice. */
  async fixChecks(worktreeId: string): Promise<FixedChecks> {
    const payload: Envelope['payload'] = { case: 'fixChecks', value: create(FixChecksSchema, { worktreeId }) };
    const key = newKey();
    const until = Date.now() + FIX_CHECKS_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, key, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      switch (reply.payload.case) {
        case 'error':
          return { why: reply.payload.value.message };
        case 'checksHandedOver':
          return { starting: reply.payload.value.starting };
        default:
          return { starting: false };
      }
    }
  }

  /** Hands a worktree's unresolved review comments to its agent, as the
   * desktop's Address comments with agent does (desktop 1.19). */
  async addressComments(worktreeId: string): Promise<AddressedComments> {
    const payload: Envelope['payload'] = { case: 'addressComments', value: create(AddressCommentsSchema, { worktreeId }) };
    const key = newKey();
    const until = Date.now() + FIX_CHECKS_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, key, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      switch (reply.payload.case) {
        case 'error':
          return { why: reply.payload.value.message };
        case 'commentsHandedOver':
          return { starting: reply.payload.value.starting };
        default:
          return { starting: false };
      }
    }
  }

  /** Merges a worktree's pull request on GitHub (desktop 1.19). `null` once
   * GitHub has it, or why it refused. Unspecified method uses the repository's
   * default of the methods it allows. */
  async mergePullRequest(worktreeId: string, method?: PullRequestMergeMethod): Promise<string | null> {
    return this.prAction(
      { case: 'mergePullRequest', value: create(MergePullRequestSchema, { worktreeId, method }) },
      'pullRequestMerged',
    );
  }

  /** Updates a worktree's pull request branch from its base (desktop 1.19). */
  async updatePullRequest(worktreeId: string): Promise<string | null> {
    return this.prAction(
      { case: 'updatePullRequest', value: create(UpdatePullRequestSchema, { worktreeId }) },
      'pullRequestUpdated',
    );
  }

  /** Marks a draft pull request ready for review (desktop 1.19). */
  async readyPullRequest(worktreeId: string): Promise<string | null> {
    return this.prAction(
      { case: 'readyPullRequest', value: create(ReadyPullRequestSchema, { worktreeId }) },
      'pullRequestReadied',
    );
  }

  /** Reopens a closed unmerged pull request (desktop 1.19). */
  async reopenPullRequest(worktreeId: string): Promise<string | null> {
    return this.prAction(
      { case: 'reopenPullRequest', value: create(ReopenPullRequestSchema, { worktreeId }) },
      'pullRequestReopened',
    );
  }

  /** Pins or unpins a worktree, as the desktop's worktree menu does (desktop
   * 1.20): a pinned one is listed first in its project. `null` once it is,
   * or the desktop's reason it is not. */
  async pinWorktree(worktreeId: string, pinned: boolean): Promise<string | null> {
    const reply = await this.send(
      { case: 'pinWorktree', value: create(PinWorktreeSchema, { worktreeId, pinned }) },
      true,
    );
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /** Renames a worktree as the desktop's worktree menu does (desktop 1.33):
   * the name ket shows for it, not its branch or folder. Empty goes back to
   * the branch's. `null` once it is, or the desktop's reason it is not. */
  async renameWorktree(worktreeId: string, name: string): Promise<string | null> {
    const reply = await this.send(
      { case: 'renameWorktree', value: create(RenameWorktreeSchema, { worktreeId, name }) },
      true,
    );
    return reply.payload.case === 'error' ? reply.payload.value.message : null;
  }

  /** Where the desktop sends this phone its notices while it is not
   * connected (desktop 1.34) — see lib/push. The newest replaces the last. */
  async registerPush(token: Uint8Array, environment: PushEnvironment, pushKey: Uint8Array, kinds: PushKind[]): Promise<void> {
    const reply = await this.send({
      case: 'registerPush',
      value: create(RegisterPushSchema, { token, environment, pushKey, kinds }),
    });
    if (reply.payload.case === 'error') throw new Error(reply.payload.value.message);
  }

  /** Tells the desktop to push this phone nothing more (desktop 1.34). */
  async unregisterPush(): Promise<void> {
    const reply = await this.send({ case: 'unregisterPush', value: create(UnregisterPushSchema, {}) });
    if (reply.payload.case === 'error') throw new Error(reply.payload.value.message);
  }

  /** Deletes a worktree — its checkout and its branch — as the desktop's
   * worktree menu does (desktop 1.20). The branch it kept because it has
   * unmerged commits, empty when that went too; or why not. Unforced, a
   * delete that would lose work is refused, saying what, with `losesWork`
   * set: `force` deletes it anyway. Any other refusal forcing would not help.
   *
   * Asked again from the next session under the same key if the connection
   * drops meanwhile, so the desktop answers with how that went. */
  async deleteWorktree(
    worktreeId: string,
    force: boolean,
  ): Promise<{ keptBranch: string } | { why: string; losesWork: boolean }> {
    const payload: Envelope['payload'] = {
      case: 'deleteWorktree',
      value: create(DeleteWorktreeSchema, { worktreeId, force }),
    };
    const key = newKey();
    const until = Date.now() + DELETE_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, key, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      if (reply.payload.case === 'error') {
        return { why: reply.payload.value.message, losesWork: reply.payload.value.code === ErrorCode.WOULD_LOSE_WORK };
      }
      if (reply.payload.case === 'worktreeDeleted') return { keptBranch: reply.payload.value.keptBranch };
      return { why: 'the desktop did not say how it went', losesWork: false };
    }
  }

  /** Since 1.23: wakes an asleep tab on the desktop — `key` from the
   * worktree's `asleep` — and resolves with the terminal it is now, or why
   * the desktop would not wake it. Asked again from the next session under
   * the same key if the connection drops meanwhile, so one tap wakes once. */
  async wakeTerminal(worktreeId: string, key: string): Promise<{ terminalId: bigint } | { why: string }> {
    const payload: Envelope['payload'] = {
      case: 'wakeTerminal',
      value: create(WakeTerminalSchema, { worktreeId, key }),
    };
    const idempotency = newKey();
    const until = Date.now() + WAKE_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, idempotency, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      if (reply.payload.case === 'error') return { why: reply.payload.value.message };
      if (reply.payload.case === 'terminalWoken') return { terminalId: reply.payload.value.terminalId };
      return { why: 'the desktop did not say whether the tab woke' };
    }
  }

  /** Since 1.28: starts terminal `terminalId`, whose session went cold, on a
   * new conversation, as the desktop's own Start fresh does — in place, or
   * from a desktop before ket 0.3 in a new terminal beside it.
   * `session` and `sinceMs` are the terminal's `cold`, so a session used
   * since is left alone. Resolves with the terminal the fresh one is in, or
   * why not.
   * `false` when the phone turns out not to have control. Asked again from
   * the next session under the same key if the connection drops meanwhile,
   * so one tap starts one session. */
  async startFresh(
    terminalId: bigint,
    session: string,
    sinceMs: bigint,
  ): Promise<{ terminalId: bigint } | { why: string } | false> {
    const payload: Envelope['payload'] = {
      case: 'startFresh',
      value: create(StartFreshSchema, { terminalId, session, sinceMs }),
    };
    const idempotency = newKey();
    const until = Date.now() + WAKE_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, idempotency, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      if (reply.payload.case === 'freshStarted') return { terminalId: reply.payload.value.terminalId };
      if (reply.payload.case !== 'error') return { why: 'the desktop did not say whether it started fresh' };
      if (reply.payload.value.code === ErrorCode.NOT_CONTROLLER) {
        const renew = this.leases.get(terminalId);
        if (renew) clearInterval(renew);
        this.leases.delete(terminalId);
        return false;
      }
      return { why: reply.payload.value.message };
    }
  }

  /** Resolves once a snapshot lists terminal `id`, or after `ms`: a woken
   * tab's terminal is in the next snapshot, a second away at most, and a
   * screen opened before then would say the terminal isn't there. */
  awaitTerminal(id: bigint, ms = 3000): Promise<void> {
    const listed = () =>
      (this.snapshot?.projects ?? []).some((project) =>
        project.worktrees.some((worktree) => worktree.terminals.some((terminal) => terminal.id === id)),
      );
    if (listed()) return Promise.resolve();
    return new Promise((resolve) => {
      const done = () => {
        off();
        clearTimeout(timer);
        resolve();
      };
      const off = this.onChange(() => {
        if (listed()) done();
      });
      const timer = setTimeout(done, ms);
    });
  }

  private async prAction(payload: Envelope['payload'], ok: Envelope['payload']['case']): Promise<string | null> {
    const key = newKey();
    const until = Date.now() + PULL_REQUEST_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, key, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      if (reply.payload.case === 'error') return reply.payload.value.message;
      if (reply.payload.case === ok) return null;
      return 'the desktop did not say how it went';
    }
  }

  /** Sends comments on a worktree's diff to its agent as one review, as the
   * desktop's Send to agent does (desktop 1.14): ket there writes the
   * message. `null` once the agent has it, or the desktop's reason it did
   * not go — a local model busy with other work among them.
   *
   * Asked again from the next session under the same key if the connection
   * drops meanwhile, so the desktop answers with how the first went rather
   * than giving the agent the review twice. */
  async sendReview(worktreeId: string, comments: ReviewNote[]): Promise<string | null> {
    const payload: Envelope['payload'] = {
      case: 'sendReview',
      value: create(SendReviewSchema, {
        worktreeId,
        comments: comments.map((comment) => create(ReviewCommentSchema, comment)),
      }),
    };
    const key = newKey();
    const until = Date.now() + REVIEW_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, key, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      return reply.payload.case === 'error' ? reply.payload.value.message : null;
    }
  }

  /** What a pull request for a worktree would be (desktop 1.16): the title
   * and description to offer, and where it would go. */
  async pullRequestDraft(worktreeId: string): Promise<PullRequestDraft> {
    const reply = await this.send({ case: 'getPullRequestDraft', value: create(GetPullRequestDraftSchema, { worktreeId }) });
    if (reply.payload.case === 'pullRequestDraft') return reply.payload.value;
    throw new Error(reply.payload.case === 'error' ? reply.payload.value.message : 'the desktop did not send the draft');
  }

  /** Commits a worktree's work, pushes its branch and opens a pull request
   * for it, as the desktop's Pull request… does (desktop 1.16) — or pushes
   * to the one already open.
   *
   * A push takes a while, and the desktop carries on with it if the
   * connection drops meanwhile. So it is asked again from the next session
   * under the same key, and the desktop answers with how that went rather
   * than pushing twice. */
  async openPullRequest(
    worktreeId: string,
    words: { title: string; body: string; draft: boolean },
  ): Promise<OpenedPullRequest> {
    const payload: Envelope['payload'] = {
      case: 'openPullRequest',
      value: create(OpenPullRequestSchema, { worktreeId, ...words }),
    };
    const key = newKey();
    const until = Date.now() + PULL_REQUEST_WAIT_MS;
    for (;;) {
      let reply: Envelope;
      try {
        reply = await this.request(payload, key, Math.max(until - Date.now(), 1));
      } catch (error) {
        const dropped = error instanceof Error && error.message === DROPPED;
        if (dropped && (await this.reconnected(until))) continue;
        throw error;
      }
      switch (reply.payload.case) {
        case 'pullRequestOpened':
          return { opened: reply.payload.value };
        case 'error':
          return { why: reply.payload.value.message };
        default:
          return { why: 'the desktop did not say how it went' };
      }
    }
  }

  /** `true` once the desktop has welcomed a new session; `false` if `until`
   * passes, or the connection is closed, first. */
  private reconnected(until: number): Promise<boolean> {
    return new Promise((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const done = (back: boolean) => {
        clearTimeout(timer);
        stop();
        resolve(back);
      };
      const stop = this.onChange(() => {
        if (this.status === 'connected') done(true);
        else if (this.closed) done(false);
      });
      timer = setTimeout(() => done(false), Math.max(until - Date.now(), 0));
      if (this.status === 'connected') done(true);
    });
  }

  /** The snippets saved in ket on the desktop. */
  async snippets(): Promise<{ name: string; body: string }[]> {
    const reply = await this.send({ case: 'getSnippets', value: create(GetSnippetsSchema, {}) });
    return reply.payload.case === 'snippets' ? reply.payload.value.snippets : [];
  }

  /** What the agent in a terminal has said since `after` — 0 for the recent
   * end of its session. Ask again with the reply's `next` for what is new. */
  async conversation(terminalId: bigint, after: bigint, session = ''): Promise<Conversation> {
    const reply = await this.send({
      case: 'getConversation',
      value: create(GetConversationSchema, { terminalId, after, session }),
    });
    if (reply.payload.case === 'conversation') return reply.payload.value;
    if (reply.payload.case === 'error' && reply.payload.value.code === ErrorCode.BACKPRESSURE) {
      throw new DesktopBusy(reply.payload.value.message);
    }
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
   * control. Resolves `null` once pressed or when that prompt is already
   * gone, otherwise the desktop's reason it could not answer.
   */
  async answer(question: { id: bigint; terminalId: bigint }, choice: Answer_Choice): Promise<string | null> {
    const reply = await this.send(
      {
        case: 'answer',
        value: create(AnswerSchema, { terminalId: question.terminalId, questionId: question.id, choice }),
      },
      true,
    );
    return this.settleQuestion(question, reply);
  }

  /**
   * Answers a question the agent asked with options, or a plan it wants to
   * go ahead with — a `Question` with `asks` or `plan` (desktop 1.8). For a
   * question, one pick per question in order: the indexes chosen, or what was
   * written instead. For a plan, approve, or keep planning with feedback.
   * The desktop hands it to the agent only if that terminal is still showing
   * that very prompt. Resolves `null` once answered or already gone,
   * otherwise the desktop's reason it could not answer.
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
    return this.settleQuestion(question, reply);
  }

  /** An accepted answer or an obsolete prompt is finished. Other refusals
   * and transport failures leave the question available for another try. */
  private settleQuestion(question: { id: bigint; terminalId: bigint }, reply: Envelope): string | null {
    if (reply.payload.case === 'error' && reply.payload.value.code !== ErrorCode.NOT_FOUND) {
      return reply.payload.value.message;
    }
    this.settledQuestions.add(`${question.terminalId}/${question.id}`);
    if (this.snapshot) this.snapshot = this.withoutSettledQuestions(this.snapshot);
    this.notify();
    return null;
  }

  /** Clear only the answered prompt, including its waiting status. A new
   * prompt in the same terminal, or another pane's prompt, stays visible. */
  private withoutSettledQuestions(snapshot: HostSnapshot, prune = false): HostSnapshot {
    if (this.settledQuestions.size === 0) return snapshot;
    const listed = new Set<string>();
    const key = (question: { id: bigint; terminalId: bigint }) => `${question.terminalId}/${question.id}`;
    const projects = snapshot.projects.map((project) => ({
      ...project,
      worktrees: project.worktrees.map((worktree) => {
        const all = [...worktree.questions, ...(worktree.question ? [worktree.question] : [])];
        for (const question of all) listed.add(key(question));
        const removed = all.filter((question) => this.settledQuestions.has(key(question)));
        if (removed.length === 0) return worktree;
        const remaining = all.filter((question) => !this.settledQuestions.has(key(question)));
        const questions = worktree.questions.filter((question) => !this.settledQuestions.has(key(question)));
        const question = worktree.question && !this.settledQuestions.has(key(worktree.question))
          ? worktree.question
          : questions[0];
        const terminals = worktree.terminals.map((terminal) =>
          terminal.activity === Activity.BLOCKED
          && removed.some((question) => question.terminalId === terminal.id)
          && !remaining.some((question) => question.terminalId === terminal.id)
          && !terminal.agentView?.sessions.some((session) => session.state === BackgroundSession_State.NEEDS_YOU)
            ? { ...terminal, activity: Activity.QUIET }
            : terminal,
        );
        const activity = worktree.activity === Activity.BLOCKED && remaining.length === 0
          ? [Activity.BLOCKED, Activity.WORKING, Activity.MERGING, Activity.RUNNING, Activity.FAILED]
            .find((activity) => terminals.some((terminal) => !terminal.closed && terminal.activity === activity)) ?? Activity.QUIET
          : worktree.activity;
        return {
          ...worktree,
          questions,
          question,
          terminals,
          activity,
          agentState: worktree.activity === Activity.BLOCKED && remaining.length === 0 ? '' : worktree.agentState,
        };
      }),
    }));
    if (prune) {
      for (const settled of this.settledQuestions) {
        if (!listed.has(settled)) this.settledQuestions.delete(settled);
      }
    }
    return { ...snapshot, projects };
  }

  /** Fits a terminal to the phone's screen, control or not; the desktop's
   * size comes back when the phone unsubscribes or goes. `false` while
   * another phone has control. */
  async resize(terminalId: bigint, cols: number, rows: number): Promise<boolean> {
    const reply = await this.send({ case: 'resize', value: create(ResizeSchema, { terminalId, cols, rows }) }, true);
    return reply.payload.case !== 'error';
  }

  /** Since 1.32: says the terminal's conversation is on screen, so what
   * its agent last said is read here, on other phones and on the desktop.
   * Needs no control, and nothing waits on it. */
  markSeen(terminalId: bigint): void {
    if (this.hostMinor < 32) return;
    void this.send({ case: 'markSeen', value: create(MarkSeenSchema, { terminalId }) }, true).catch(() => {});
  }

  /** Marks a worktree unread, as the desktop's worktree menu does, or with
   * `unread` false takes the mark off as one of its terminals is opened
   * (desktop 1.35). */
  markUnread(worktreeId: string, unread: boolean): void {
    if (this.hostMinor < 35) return;
    void this.send({ case: 'markUnread', value: create(MarkUnreadSchema, { worktreeId, unread }) }, true).catch(() => {});
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
    if (this.snapshotDraw) clearTimeout(this.snapshotDraw);
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

/** The connection to host `id` this app already holds, if one has been made:
 * a screen opened from Worktrees has it on its first draw, rather than after
 * the Keychain gives the saved host back. */
export function liveConnection(id: string): HostConnection | undefined {
  return connections.get(id);
}

/** Every connection this app holds now, oldest first. Only a saved desktop
 * gets one, and a forgotten desktop's is dropped, so these are paired
 * desktops — for a screen's first draw, before the Keychain gives the list
 * back. */
export function heldConnections(): HostConnection[] {
  return [...connections.values()];
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
