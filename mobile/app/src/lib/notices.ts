// Notices: a banner on the phone when, on a paired desktop, an agent stops
// on a prompt, a task finishes or fails, or a pull request's checks fail —
// while ket is not the app in front. The desktop's notify.rs, on the phone,
// under Settings → Notifications.
//
// The phone posts these itself, from the snapshots it keeps receiving: a
// notice is local, not pushed. iOS keeps the connection alive for about half
// a minute after the app leaves the screen, so news from later waits for the
// next open. A push service carrying the same words through APNs would plug
// in here, under the same switches.
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
import { router } from 'expo-router';
import { useEffect, useState } from 'react';
import { AppState, Linking, Platform } from 'react-native';

import { agentOf, everyWorktree } from './activity';
import { connection, type HostConnection } from './connection';
import { DEFAULT_NOTIFY, type NotifyConfig, getNotify, listHosts, saveNotify } from './store';
import { agentLabel } from './theme';

/** Whether this phone can show notices at all: a browser cannot. */
export const available = Platform.OS !== 'web';

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
  if (!available) return;
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
}

async function ask(): Promise<boolean> {
  if (!available) return false;
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
    const path = question?.terminalId
      ? `/session/${host.id}/${question.terminalId.toString()}`
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
  if (!available || !state.config.on || state.refused) return;
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

/** Takes in a desktop's snapshot: posts the news, takes back the answered. */
async function look(conn: HostConnection, named: boolean): Promise<void> {
  const { fresh, answered } = compare(conn.host, conn.snapshot, named);
  if (!available) return;
  for (const id of answered) void Notifications.dismissNotificationAsync(id).catch(() => {});
  const { config, refused } = state;
  if (!config.on || refused) return;
  if (AppState.currentState !== 'active') {
    for (const notice of fresh) if (config[notice.kind]) await post(notice);
  }
  await refreshBadge();
}

/** A tapped notice opens what it is about. */
function open(response: Notifications.NotificationResponse | null | undefined): void {
  const path = response?.notification.request.content.data?.path;
  if (typeof path === 'string' && path.startsWith('/')) router.push(path);
}

/** How often the paired desktops are listed again, so one paired since the
 * app started is followed too. */
const RELIST_MS = 5000;

/** Reads the switches and starts following every paired desktop. Called once,
 * when the app starts. */
export function startNotices(): void {
  void getNotify().then(async (config) => {
    let refused = false;
    if (available && config.on) refused = !(await Notifications.getPermissionsAsync()).granted;
    publish({ config, refused });
    await refreshBadge();
  });
  if (!available) return;
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
}

/** Follows the paired desktops' snapshots for news, and opens what a tapped
 * notice is about — the one the app was opened with, too. */
export function useNoticeFollowing(): void {
  const response = Notifications.useLastNotificationResponse();
  useEffect(() => {
    if (response) open(response);
  }, [response]);
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
          const named = conns.length > 1;
          offs = conns.map((c) => c.onChange(() => void look(c, named)));
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
    };
  }, []);
}
