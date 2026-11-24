// Notices: a banner on the phone when, on a paired desktop, an agent stops
// on a prompt, a task finishes or fails, or a pull request's checks fail —
// while ket is not the app in front. The desktop's notify.rs, on the phone,
// under Settings → Notifications.
//
// The phone posts these itself, from the snapshots it keeps receiving. iOS
// keeps the connection alive for about half a minute after the app leaves the
// screen; from then on a desktop from 1.34 pushes the same notices through
// APNs, under the same switches — see lib/push.ts. A pushed notice that is no
// longer so — its prompt answered, its finished task read, its checks passing
// — is taken back once the phone hears from that desktop again.
//
// Only the move into a state is announced: a worktree that stays waiting is
// not announced again, one that stops and starts again is, and nothing is
// said while ket is in front, where the row says it already. A notice whose
// prompt has been answered is taken back. The first snapshot from each
// desktop only learns what was already so: those are not news. The app's
// badge counts the prompts waiting, across every desktop.

import { Activity, CheckSummary_State, type HostSnapshot, type WorktreeSummary } from '@ket/remote';
import * as Notifications from 'expo-notifications';
import { AndroidImportance } from 'expo-notifications';
import { useEffect, useState } from 'react';
import { AppState, Linking, Platform } from 'react-native';

import { agentOf, agentTerminal, everyWorktree } from './activity';
import { connection, type HostConnection } from './connection';
import { showOverWorktrees } from './layout';
import { type Pushed, forgetToken, pushable, pushedOf, pushedPath, syncPush } from './push';
import { DEFAULT_NOTIFY, type NotifyConfig, getNotify, listHosts, saveNotify } from './store';
import { agentLabel } from './theme';

/** The three kinds of notice, each with its own switch. */
export type Kind = 'waiting' | 'finished' | 'checks';

/** What Settings sees. */
export interface NoticeState {
  config: NotifyConfig;
  /** The phone will not show ket's notices: they are on here, but turned
   * off for ket in the phone's own Settings, where they are given back. */
  refused: boolean;
}

let state: NoticeState = { config: DEFAULT_NOTIFY, refused: false };
const listeners = new Set<() => void>();

function publish(next: NoticeState): void {
  state = next;
  listeners.forEach((fn) => fn());
}

export function useNotices(): NoticeState {
  const [, redraw] = useState(0);
  useEffect(() => {
    const fn = () => redraw((n) => n + 1);
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  }, []);
  return state;
}

/** Turns notices on, asking the phone first the first time. `false` when
 * the phone refuses: they are left on, so the switch shows what to fix. */
export async function turnOn(): Promise<boolean> {
  const granted = await ask();
  await keep({ ...state.config, on: true }, !granted);
  if (granted) await refreshBadge();
  return granted;
}

/** Turns them off, taking back the ones showing and the badge. */
export async function turnOff(): Promise<void> {
  await keep({ ...state.config, on: false }, false);
  await Notifications.dismissAllNotificationsAsync().catch(() => {});
  await Notifications.setBadgeCountAsync(0).catch(() => {});
}

export async function choose(kind: Kind, on: boolean): Promise<void> {
  await keep({ ...state.config, [kind]: on }, state.refused);
}

/** The phone's own Settings page for ket, where refused notices are given back. */
export function openPhoneSettings(): void {
  void Linking.openSettings();
}

async function keep(config: NotifyConfig, refused: boolean): Promise<void> {
  await saveNotify(config);
  publish({ config, refused });
  tellDesktops();
}

/** The desktops followed now — see `useNoticeFollowing`. */
let followed: HostConnection[] = [];

/** What push is to carry: what the switches say, and nothing while the
 * phone refuses to show it. */
function wanted() {
  const { config, refused } = state;
  return { ...config, on: config.on && !refused };
}

/** Tells every connected desktop what this phone wants pushed, after a
 * switch changed or iOS handed out a new token. */
function tellDesktops(): void {
  for (const conn of followed) void syncPush(conn, wanted());
}

async function ask(): Promise<boolean> {
  const had = await Notifications.getPermissionsAsync();
  if (had.granted) return true;
  if (!had.canAskAgain) return false;
  const asked = await Notifications.requestPermissionsAsync({
    ios: { allowAlert: true, allowBadge: true, allowSound: true },
  });
  return asked.granted;
}

/** What was known of a worktree at the last snapshot. */
interface Seen {
  activity: Activity;
  /** The agent it had then, for a notice after it has gone quiet. */
  agent: string;
  checksFailed: boolean;
}

/** Per desktop: what its worktrees were doing, once a snapshot has been seen. */
const seen = new Map<string, Map<string, Seen>>();

/** `${host}/${worktree}`: a notice's identity, and a worktree's in `seen`. */
const keyOf = (host: string, worktree: string) => `${host}/${worktree}`;

const MOVING = new Set<Activity>([Activity.WORKING, Activity.RUNNING, Activity.MERGING, Activity.BLOCKED]);

function checksFailed(worktree: WorktreeSummary): boolean {
  return (worktree.pullRequest?.checks ?? []).some((check) => check.state === CheckSummary_State.FAILED);
}

/** One notice, in the words it is shown with. */
export interface Notice {
  kind: Kind;
  id: string;
  title: string;
  subtitle?: string;
  body: string;
  /** Where a tap goes. */
  path: string;
}

/** The notices a desktop's new snapshot earns, and the waiting ones it takes
 * back. Pure, so it can be tried without a phone. `named` adds the
 * desktop's name when more than one is paired. */
export function compare(
  host: { id: string; name: string },
  snapshot: HostSnapshot | null,
  named: boolean,
): { fresh: Notice[]; answered: string[] } {
  const fresh: Notice[] = [];
  const answered: string[] = [];
  const before = seen.get(host.id);
  const now = new Map<string, Seen>();
  for (const { project, worktree } of everyWorktree(snapshot)) {
    const key = keyOf(host.id, worktree.id);
    const agent = agentOf(worktree) || before?.get(key)?.agent || '';
    const failed = checksFailed(worktree);
    now.set(key, { activity: worktree.activity, agent, checksFailed: failed });
    const was = before?.get(key);
    if (!before) continue;
    const title = `${project.name} · ${worktree.name}`;
    const subtitle = named ? host.name : undefined;
    const who = agent ? agentLabel(agent) : 'An agent';
    const question = worktree.question;
    // A tap opens the chat: the newest prompt's — a dialog read off the
    // screen is only in the list — else the agent's, the one that finished.
    // The worktree's own screen only with no agent's terminal to open.
    const asking = worktree.questions[0] ?? question;
    const chat = asking?.terminalId || agentTerminal(worktree)?.id;
    const path = chat
      ? `/session/${host.id}/${chat.toString()}`
      : `/worktree/${host.id}/${encodeURIComponent(worktree.id)}`;
    if (worktree.activity === Activity.BLOCKED && was?.activity !== Activity.BLOCKED) {
      const subject = question?.subject ? `: ${question.subject}` : '.';
      fresh.push({ kind: 'waiting', id: key, title, subtitle, body: `${who} is waiting on you${subject}`, path });
    } else if (worktree.activity !== Activity.BLOCKED && was?.activity === Activity.BLOCKED) {
      answered.push(key);
    }
    if (was && MOVING.has(was.activity) && was.agent && worktree.activity === Activity.QUIET) {
      const said = worktree.agentState ? `: ${worktree.agentState}` : '.';
      fresh.push({ kind: 'finished', id: `${key}/finished`, title, subtitle, body: `${who} finished${said}`, path });
    }
    if (was && was.activity !== Activity.FAILED && worktree.activity === Activity.FAILED) {
      fresh.push({ kind: 'finished', id: `${key}/finished`, title, subtitle, body: `${who} failed.`, path });
    }
    if (was && !was.checksFailed && failed) {
      const names = (worktree.pullRequest?.checks ?? [])
        .filter((check) => check.state === CheckSummary_State.FAILED)
        .map((check) => check.name);
      fresh.push({
        kind: 'checks',
        id: `${key}/checks`,
        title,
        subtitle,
        body: `Checks failed on #${worktree.pullRequest?.number.toString() ?? '?'}: ${names.join(', ')}`,
        path: `/pull-request/${host.id}/${encodeURIComponent(worktree.id)}`,
      });
    }
  }
  // A worktree gone from the desktop takes its prompt with it.
  for (const key of before?.keys() ?? []) {
    if (!now.has(key) && before?.get(key)?.activity === Activity.BLOCKED) answered.push(key);
  }
  seen.set(host.id, now);
  return { fresh, answered };
}

/** Forgets what was seen, for a test. */
export function forgetSeen(): void {
  seen.clear();
}

/** How many prompts are waiting across every desktop, from what was last seen. */
function waitingCount(): number {
  let n = 0;
  for (const worktrees of seen.values()) {
    for (const was of worktrees.values()) if (was.activity === Activity.BLOCKED) n += 1;
  }
  return n;
}

async function refreshBadge(): Promise<void> {
  if (!state.config.on || state.refused) return;
  await Notifications.setBadgeCountAsync(waitingCount()).catch(() => {});
}

/** Android shows nothing without a channel; iOS ignores it. */
const CHANNEL = 'needs-you';

async function post(notice: Notice): Promise<void> {
  await Notifications.scheduleNotificationAsync({
    identifier: notice.id,
    content: {
      title: notice.title,
      subtitle: notice.subtitle ?? null,
      body: notice.body,
      data: { path: notice.path },
      sound: true,
    },
    trigger: Platform.OS === 'android' ? { channelId: CHANNEL } : null,
  }).catch(() => {});
}

/** Takes in a desktop's snapshot: posts the news, takes back the answered.
 * Called for every snapshot as it comes, not only those drawn: screens draw
 * a run of them as one (see `drawSnapshot` in lib/connection), and an agent
 * that worked and went quiet between two draws still finished. */
async function take(conn: HostConnection, named: boolean): Promise<void> {
  const { fresh, answered } = compare(conn.state.host, conn.state.snapshot, named);
  for (const id of answered) void Notifications.dismissNotificationAsync(id).catch(() => {});
  const { config, refused } = state;
  if (!config.on || refused) return;
  if (AppState.currentState !== 'active') {
    for (const notice of fresh) if (config[notice.kind]) await post(notice);
  }
}

/** `take`, and what needs doing only as often as the connection changes:
 * telling the desktop what to push, taking back what it pushed, the badge. */
async function look(conn: HostConnection, named: boolean): Promise<void> {
  void syncPush(conn, wanted());
  void clearPushed(conn);
  await take(conn, named);
  await refreshBadge();
}

/** How often a desktop's pushed notices are checked against what it says. */
const CLEAR_EVERY_MS = 5000;
const clearedAt = new Map<string, number>();

/** Takes back the notices `conn`'s desktop pushed that are no longer so, as
 * it would not have sent them: the prompt answered, the finished task read
 * at the Mac or here, the checks passing again, the worktree gone. */
async function clearPushed(conn: HostConnection): Promise<void> {
  const snapshot = conn.state.snapshot;
  if (!pushable || !snapshot) return;
  const now = Date.now();
  if (now - (clearedAt.get(conn.id) ?? 0) < CLEAR_EVERY_MS) return;
  clearedAt.set(conn.id, now);
  const shown = await Notifications.getPresentedNotificationsAsync().catch(() => []);
  for (const notification of shown) {
    const pushed = pushedOf(notification.request.content.data);
    if (pushed?.host !== conn.id || stillSo(pushed, snapshot)) continue;
    void Notifications.dismissNotificationAsync(notification.request.identifier).catch(() => {});
  }
}

/** The desktop's own rule for whether a notice still stands (`still` in
 * notices.rs). */
function stillSo(pushed: Pushed, snapshot: HostSnapshot): boolean {
  const worktree = everyWorktree(snapshot).find((x) => x.worktree.id === pushed.worktree)?.worktree;
  if (!worktree) return false;
  switch (pushed.kind) {
    case 'waiting':
      return worktree.activity === Activity.BLOCKED;
    case 'finished':
      return !MOVING.has(worktree.activity) && worktree.terminals.some((terminal) => terminal.unread);
    case 'failed':
      return worktree.activity === Activity.FAILED;
    case 'checks':
      return checksFailed(worktree);
  }
}

/** The tapped notices already opened: the listener and the last response
 * both hand over the same tap, and a second push stacked a second chat. */
const opened = new Set<string>();

/** A tapped notice opens what it is about, over Worktrees. */
function open(response: Notifications.NotificationResponse | null | undefined): void {
  if (!response) return;
  const tap = `${response.notification.request.identifier}@${response.notification.date}`;
  if (opened.has(tap)) return;
  const data = response.notification.request.content.data;
  // A pushed notice, opened by the notification extension, names its
  // desktop: it opens only one this phone is still paired with.
  const pushed = pushedOf(data);
  if (pushed) {
    opened.add(tap);
    void listHosts()
      .then((hosts) => {
        if (hosts.some((host) => host.id === pushed.host)) showOverWorktrees(pushedPath(pushed));
      })
      .catch(() => {});
    return;
  }
  const path = data?.path;
  if (typeof path !== 'string' || !path.startsWith('/')) return;
  opened.add(tap);
  showOverWorktrees(path);
}

/** How often the paired desktops are listed again, so one paired since the
 * app started is followed too. */
const RELIST_MS = 5000;

/** Reads the switches and starts following every paired desktop. Called once,
 * when the app starts. */
export function startNotices(): void {
  void getNotify().then(async (config) => {
    const refused = config.on && !(await Notifications.getPermissionsAsync()).granted;
    publish({ config, refused });
    await refreshBadge();
  });
  Notifications.setNotificationHandler({
    // In front, the rows say it already: a notice landing then is kept in
    // the list without a banner.
    handleNotification: async () => ({
      shouldShowBanner: false,
      shouldShowList: true,
      shouldPlaySound: false,
      shouldSetBadge: false,
    }),
  });
  if (Platform.OS === 'android') {
    void Notifications.setNotificationChannelAsync(CHANNEL, {
      name: 'Needs you',
      importance: AndroidImportance.HIGH,
    });
  }
  Notifications.addNotificationResponseReceivedListener(open);
  // iOS hands out a new token now and then: every desktop is told it.
  if (pushable) {
    Notifications.addPushTokenListener(() => {
      forgetToken();
      tellDesktops();
    });
  }
}

/** Follows the paired desktops' snapshots for news, and opens what a tapped
 * notice is about — the one the app was opened with, too, once `ready`: a
 * push before the app's screens are there is dropped, and it opened on
 * whatever screen it started with. */
export function useNoticeFollowing(ready: boolean): void {
  const response = Notifications.useLastNotificationResponse();
  useEffect(() => {
    if (ready) open(response);
  }, [response, ready]);
  useEffect(() => {
    let live = true;
    let ids = '';
    let offs: (() => void)[] = [];
    const relist = () => {
      void listHosts()
        .then((hosts) => {
          if (!live) return;
          const next = hosts.map((host) => host.id).join(',');
          if (next === ids) return;
          ids = next;
          offs.forEach((off) => off());
          const conns = hosts.map((host) => connection(host));
          followed = conns;
          const named = conns.length > 1;
          offs = conns.flatMap((c) => [
            c.onChange(() => void look(c, named)),
            c.onMessage((message) => {
              if (message.payload.case === 'snapshot') void take(c, named);
            }),
          ]);
          conns.forEach((c) => void look(c, named));
        })
        .catch(() => {
          // The Keychain unreadable for now: the next look tries again.
        });
    };
    relist();
    const timer = setInterval(relist, RELIST_MS);
    return () => {
      live = false;
      clearInterval(timer);
      offs.forEach((off) => off());
      followed = [];
    };
  }, []);
}
