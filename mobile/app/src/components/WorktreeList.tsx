// Worktrees: the desktop sidebar, on a phone. Along the top, the desktop's
// name as a large title over a line saying the link is encrypted and how it
// runs, with Start, which opens the New task sheet, across from them; under
// them the sidebar's filter as a row of tabs, then what waits on you and each
// project, its name on the desk over a ruled card of its worktrees. The cards
// scroll under the tab bar, which floats over the desk's foot, and come to
// rest above it.
//
// Holding a project offers work started in it, its backlog, pinning it to
// the top and hiding it — both this phone's own. A hidden project is back
// while an agent in it waits on you; the rest wait behind a line under the
// cards.
//
// A worktree at work opens up in the list: what its agent is doing, in a
// terminal's well, with the lines it has changed. Holding any worktree opens
// its menu — what the desktop's worktree menu offers that the phone can do:
// pin it, act on its pull request, delete it. Never the repository's own
// checkout's pin or delete.
//
// On a phone it is the Worktrees tab. On a wide screen it is the sidebar,
// always drawn, and what a row opens goes beside it (see lib/layout.ts).

import { Activity, Answer_Choice, type ProjectSummary, type WorktreeSummary } from '@ket/remote';
import { router, useFocusEffect, useGlobalSearchParams, usePathname } from 'expo-router';
import { type ComponentType, type ReactNode, createElement, useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Pressable, ScrollView, StyleSheet, Text, View, type ViewInstance } from 'react-native';
import Animated, { FadeIn, ReduceMotion } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { AutomationsSheet } from './AutomationsSheet';
import { Connecting } from './Connecting';
import { ListSkeleton } from './ListSkeleton';
import { Track } from './Track';
import type { AddressKind } from '../lib/address-kind';
import { FloatMenu, type MenuItem, type MenuSection, type Place, placeOf } from './FloatMenu';
import { Seal } from './Seal';
import { Sheet, SheetItem, SheetList, SheetNote } from './Sheet';
import { Unpaired } from './Unpaired';
import {
  Bolt,
  Bubble,
  Chevron,
  ChevronDown,
  Clock,
  Desktop,
  EyeOff,
  ListTodo,
  Merge,
  Pin,
  PinOff,
  Plus,
  Sparkle,
  Wildcard,
} from './icons';
import {
  AgentTile,
  Badge,
  Button,
  Pill,
  ProjectHeading,
  Prose,
  Row,
  RuledCard,
  Screen,
  SectionLabel,
  TAB_BAR_HEIGHT,
  TabBar,
  Tabs,
  tabBarFoot,
} from './ui';
import {
  type Filter,
  agentOf,
  agentTerminal,
  asleepTab,
  colorOf,
  detail,
  everyWorktree,
  isUnread,
  passes,
  projectColor,
  projectKey,
  promptKind,
  questionsOf,
} from '../lib/activity';
import { elapsed, firstLine, progressOf, stageText, startingAt } from '../lib/starting';
import type { HostConnection } from '../lib/connection';
import { menuOpened } from '../lib/haptics';
import { statusLine, usePairedHosts } from '../lib/hosts';
import { showBeside, tabAt, worktreeAt } from '../lib/layout';
import { settlesAt, settling } from '../lib/settling';
import {
  getCollapsed,
  getDesktop,
  getHiddenProjects,
  getPinnedProjects,
  saveCollapsed,
  saveDesktop,
  saveHiddenProjects,
  savePinnedProjects,
} from '../lib/store';
import { agentLabel, font, size, t, tint } from '../lib/theme';
import { surface } from './ui/surface';
import { worktreeMenu } from './WorktreeMenu';
import { WorktreeRow } from './WorktreeRow';

/** Between the last card, at rest, and the tab bar under it. */
const FOOT_GAP = 10;

/** The screen's edge: the desk's gutter round the cards, and the header's. */
const EDGE = 16;

/** How the link to the desktop runs, after "Secure" under its name. */
const ROUTE_WORD: Record<AddressKind, string> = {
  local: 'Wi-Fi',
  tailnet: 'Tailscale',
  'ket-relay': 'Relay',
  relay: 'Relay',
};

/** What a filter with nothing under it says. */
const EMPTY: Record<Exclude<Filter, 'all'>, string> = {
  active: 'Nothing is running.',
  unread: 'Nothing unread.',
  ready: 'Nothing ready to land.',
};

/** The list coming in over its skeleton. */
const FADE_IN = FadeIn.duration(220).reduceMotion(ReduceMotion.System);

/** The worktree list. `sidebar` draws it as the wide screen's sidebar: the
 * open worktree marked, and every row opening beside it. `refresh` re-reads
 * the paired desktops and the chosen one when it changes — the sidebar is
 * never focused anew the way a tab is, so the layout passes the path. */
export function WorktreeList({ sidebar = false, refresh }: { sidebar?: boolean; refresh?: string }) {
  const hosts = usePairedHosts(refresh);
  const path = usePathname();
  // A starting screen that has become its terminal's chat names it here.
  const { terminal: shownTerminal } = useGlobalSearchParams<{ terminal?: string }>();
  const insets = useSafeAreaInsets();
  // Navigate, not push: a second tap on a row, landing once its screen is
  // already on top, opens nothing more. A copy further down is not reused.
  const open = (href: string) => (sidebar ? showBeside(href) : router.navigate(href));
  const [chosen, setChosen] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
  // Rows opened from under Unread: read now, but kept where they were until
  // the filter changes, so the list does not close up as you come back.
  const [stay, setStay] = useState<string[]>([]);
  const pickFilter = (next: Filter) => {
    setFilter(next);
    setStay([]);
  };
  // The Mac switcher, a sheet, while it is up.
  const [picking, setPicking] = useState(false);
  // The projects hidden on this phone, a sheet, while it is up.
  const [tucking, setTucking] = useState(false);
  const [automating, setAutomating] = useState(false);
  // Automations, asked for from the Macs sheet: it rises once that sheet's
  // modal is gone, since iOS shows one at a time.
  const [toAutomations, setToAutomations] = useState(false);
  const [menu, setMenu] = useState<{ worktree: WorktreeSummary; place: Place } | null>(null);
  const [projectMenu, setProjectMenu] = useState<{ project: ProjectSummary; place: Place } | null>(null);
  // Worktrees being deleted from here: out of the list from the moment
  // Delete is confirmed, since the desktop takes a while to walk the
  // checkout and ask git, and kept out until its next snapshot leaves them
  // out too. Back in it if the delete is refused.
  const [gone, setGone] = useState<string[]>([]);
  // Worktrees whose asleep tab the desktop is waking for this phone.
  const [waking, setWaking] = useState<string[]>([]);
  // Each worktree's row, and each project's heading, to find where it is
  // when it is held.
  const held = useRef(new Map<string, ViewInstance>());
  const approving = useRef(new Set<string>());

  const readChosen = useCallback(() => {
    void getDesktop().then(setChosen);
  }, []);
  useFocusEffect(readChosen);
  useEffect(() => {
    if (refresh !== undefined) readChosen();
  }, [refresh, readChosen]);

  const current = hosts?.find((h) => h.conn.id === chosen) ?? hosts?.[0] ?? null;
  const conn = current?.conn ?? null;
  const hostState = current?.hostState ?? null;
  // Work this phone asked for counts the seconds it has waited, in its row.
  const anyStarting = (hostState?.starting.length ?? 0) > 0;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!anyStarting) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [anyStarting]);
  const hostId = conn?.id ?? null;
  // The list while it fills: from the desktop's welcome until its list is
  // here, and a moment longer when that list is empty — see lib/settling.
  // `now` can be older than the welcome; the clock is read again when the
  // wait ends, or at once if it already has, to show what is there.
  const filling = hostState !== null && settling(hostState, now);
  const until = filling && hostState ? settlesAt(hostState) : null;
  useEffect(() => {
    if (until === null) return;
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, until - Date.now()) + 20);
    return () => clearTimeout(timer);
  }, [until]);

  // Folded projects, kept per desktop on the phone. Held with the desktop they
  // were read for, so switching desktops never shows the last one's folds.
  const [folds, setFolds] = useState<{ host: string; projects: string[] } | null>(null);
  useEffect(() => {
    if (hostId) void getCollapsed(hostId).then((projects) => setFolds({ host: hostId, projects }));
  }, [hostId]);
  const collapsed = folds && folds.host === hostId ? folds.projects : [];

  // A worktree that arrives in a folded project unfolds it: new work is
  // what a person looks for, and a folded heading no longer says how much
  // is under it. The first list a desktop sends is only what was there.
  const seen = useRef<{ host: string; ids: Set<string> } | null>(null);
  const snapshot = hostState?.snapshot ?? null;
  useEffect(() => {
    if (!hostId || !snapshot || !folds || folds.host !== hostId) return;
    const ids = new Set(everyWorktree(snapshot).map((l) => l.worktree.id));
    const was = seen.current?.host === hostId ? seen.current.ids : null;
    seen.current = { host: hostId, ids };
    if (!was) return;
    const arrived = snapshot.projects
      .filter((project) => project.worktrees.some((w) => !was.has(w.id)))
      .map(projectKey);
    const unfold = arrived.filter((key) => folds.projects.includes(key));
    if (unfold.length === 0) return;
    const next = folds.projects.filter((key) => !unfold.includes(key));
    setFolds({ host: hostId, projects: next });
    void saveCollapsed(hostId, next);
  }, [hostId, snapshot, folds]);
  const toggle = (project: string) => {
    if (!hostId) return;
    const next = collapsed.includes(project) ? collapsed.filter((p) => p !== project) : [...collapsed, project];
    setFolds({ host: hostId, projects: next });
    void saveCollapsed(hostId, next);
  };
  // Projects hidden from the list, and pinned to its top: the phone's own,
  // per desktop, and held with the desktop they were read for as folds are.
  const [kept, setKept] = useState<{ host: string; hidden: string[]; pinned: string[] } | null>(null);
  useEffect(() => {
    if (!hostId) return;
    void Promise.all([getHiddenProjects(hostId), getPinnedProjects(hostId)]).then(([hidden, pinned]) =>
      setKept({ host: hostId, hidden, pinned }),
    );
  }, [hostId]);
  const hiddenKeys = kept && kept.host === hostId ? kept.hidden : [];
  const pinnedKeys = kept && kept.host === hostId ? kept.pinned : [];
  const keep = (hidden: string[], pinned: string[]) => {
    if (!hostId) return;
    setKept({ host: hostId, hidden, pinned });
    if (hidden !== hiddenKeys) void saveHiddenProjects(hostId, hidden);
    if (pinned !== pinnedKeys) void savePinnedProjects(hostId, pinned);
  };

  const waitingEverywhere = (hosts ?? []).reduce(
    (n, { hostState: h }) => n + everyWorktree(h.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );

  if (hosts !== null && hosts.length === 0) {
    return (
      <Screen>
        <Unpaired />
        <TabBar active="worktrees" needs={0} />
      </Screen>
    );
  }

  // The projects listed: pinned first, then in the desktop's order, without
  // the hidden — unless an agent in one waits on you, which brings it back
  // until that is answered.
  const waitsOnYou = (project: ProjectSummary) => project.worktrees.some((w) => w.activity === Activity.BLOCKED);
  const hiddenHere = (project: ProjectSummary) => hiddenKeys.includes(projectKey(project));
  const pinnedHere = (project: ProjectSummary) => pinnedKeys.includes(projectKey(project));
  const projects = (hostState?.snapshot?.projects ?? [])
    .filter((project) => !hiddenHere(project) || waitsOnYou(project))
    .sort((a, b) => Number(pinnedHere(b)) - Number(pinnedHere(a)));
  const tucked = (hostState?.snapshot?.projects ?? []).filter(hiddenHere);
  const tuckedAway = tucked.filter((project) => !waitsOnYou(project)).length;
  const located = projects.flatMap((project) => project.worktrees.map((worktree) => ({ project, worktree })));
  const all = located.map((l) => l.worktree);
  const count = (f: Filter) => all.filter((w) => passes(w, f)).length;
  // No counts while the list fills: a row of zeros would say nothing runs.
  const tally = (f: Filter) => (filling ? undefined : count(f));
  const named = (word: string, f: Filter) => (filling ? word : `${word}, ${count(f)}`);
  // Nothing at all on the desktop, now that its list has settled.
  const empty = !filling && hostState?.status === 'connected' && hostState.snapshot !== null && hostState.snapshot.projects.length === 0;
  // Every prompt waiting on you, a row each, over the projects.
  const waiting = located
    .filter(({ worktree }) => worktree.activity === Activity.BLOCKED)
    .flatMap((row) => questionsOf(row.worktree).map((question, at) => ({ ...row, question, at })));
  type Waiting = (typeof waiting)[number];
  // What holding a prompt's row does: allow it once, or approve the plan. A
  // question with answers of its own is answered in its chat.
  const holdLabel = ({ question }: Waiting) => {
    const kind = question ? promptKind(question.tool) : undefined;
    if (!question?.answerable || !question.id || !question.terminalId || kind === 'ask') return undefined;
    return kind === 'plan' ? 'Approve plan' : 'Allow once';
  };
  // A changed prompt remounts its row, cancelling any hold in progress.
  const waitingKey = (w: Waiting) =>
    `waiting/${hostId}/${w.worktree.id}/${w.question?.terminalId}/${w.question?.id ?? w.at}/${holdLabel(w)}`;
  const status = statusLine(hostState);
  const statusWords = status.connected
    ? status.kind
      ? `Secure · ${ROUTE_WORD[status.kind]}`
      : 'Secure'
    : hostState?.status === 'offline'
      ? 'Offline'
      : 'Reaching';
  // Reaching for the desktop: its worktrees are not known yet, so the screen
  // shows the reaching instead of an empty list.
  const down = hostState !== null && hostState.status !== 'connected';
  // What the last card comes to rest above: the tab bar floating over the
  // desk, and a gutter between the two.
  const under = tabBarFoot(insets.bottom) + TAB_BAR_HEIGHT + FOOT_GAP;

  // The worktree open beside the sidebar.
  const opened =
    sidebar && conn && hostState
      ? (worktreeAt(path, conn.id, hostState.snapshot) ??
        startingAt(path, conn.id, hostState.asked, hostState.snapshot, shownTerminal))
      : null;

  const choose = (c: HostConnection) => {
    setChosen(c.id);
    void saveDesktop(c.id);
  };

  // A prompt's chat, where it is answered.
  const answer = ({ worktree, question }: Waiting) => {
    if (!conn) return;
    open(
      question?.terminalId
        ? `/session/${conn.id}/${question.terminalId}`
        : `/worktree/${conn.id}/${encodeURIComponent(worktree.id)}`,
    );
  };

  const approve = async (w: Waiting) => {
    const label = holdLabel(w);
    const key = waitingKey(w);
    if (!conn || !w.question || !label || approving.current.has(key)) return;
    approving.current.add(key);
    const request =
      label === 'Approve plan'
        ? conn.respond(w.question, { approve: true })
        : conn.answer(w.question, Answer_Choice.ALLOW_ONCE);
    const refused = await request.catch((error: unknown) => (error instanceof Error ? error.message : String(error)));
    approving.current.delete(key);
    if (refused) Alert.alert('Couldn’t answer the prompt', refused);
  };

  // Straight into the work: the agent's conversation, or with no agent, the
  // first terminal still open.
  const follow = (worktree: WorktreeSummary) => {
    if (!conn) return;
    if (filter === 'unread' && !stay.includes(worktree.id)) setStay([...stay, worktree.id]);
    const agentTerm = agentTerminal(worktree);
    if (agentTerm) {
      open(`/session/${conn.id}/${agentTerm.id.toString()}`);
      return;
    }
    // Claude's agent view in a plain shell tab — `claude agents` typed into
    // it — has no agent of its own; its sessions list is still the place.
    const view = worktree.terminals.find((x) => !x.closed && x.agentView);
    if (view) {
      open(`/session/${conn.id}/${view.id.toString()}`);
      return;
    }
    const running = worktree.terminals.find((x) => !x.closed);
    if (running) {
      open(`/terminal/${conn.id}/${running.id.toString()}`);
      return;
    }
    // An agent tab the desktop put to sleep: woken there, then its chat —
    // before a terminal that has exited, which has nothing left to show.
    if (conn.state.hostMinor >= 23 && worktree.asleep.length > 0) {
      void wake(worktree);
      return;
    }
    const exited = worktree.terminals[0];
    if (exited) {
      open(`/terminal/${conn.id}/${exited.id.toString()}`);
      return;
    }
    // No terminal at all — work Claude's agent view runs there under its own
    // daemon has none: the worktree's own screen, rather than a tap that does
    // nothing.
    open(`/worktree/${conn.id}/${encodeURIComponent(worktree.id)}`);
  };

  // Wakes the worktree's asleep tab on the desktop and opens its chat once
  // its terminal is there. The desktop says why when it won't.
  const wake = async (worktree: WorktreeSummary) => {
    const tab = asleepTab(worktree);
    if (!conn || !tab || waking.includes(worktree.id)) return;
    setWaking((was) => [...was, worktree.id]);
    const woke = await conn
      .wakeTerminal(worktree.id, tab.key)
      .catch((e: unknown) => ({ why: e instanceof Error ? e.message : String(e) }));
    setWaking((was) => was.filter((id) => id !== worktree.id));
    if ('why' in woke) {
      Alert.alert('Couldn’t wake the agent', woke.why);
      return;
    }
    await conn.awaitTerminal(woke.terminalId);
    open(`/session/${conn.id}/${woke.terminalId.toString()}`);
  };

  const going = (id: string, away: boolean) =>
    setGone((was) => (away ? (was.includes(id) ? was : [...was, id]) : was.filter((x) => x !== id)));
  const sections = menu && conn && hostState ? worktreeMenu(conn, hostState, menu.worktree, open, going) : [];
  const hold = (worktree: WorktreeSummary) => {
    if (!conn || worktreeMenu(conn, conn.state, worktree, open, going).length === 0) return;
    placeOf(held.current.get(worktree.id), (place) => {
      menuOpened();
      setMenu({ worktree, place });
    });
  };

  // What holding a project offers: work started in it and its backlog —
  // which holding its heading opened before there was a menu — then pinning
  // it to the top of the list, then hiding it. The terminals in no project
  // have no work to start or backlog.
  const projectSections = (project: ProjectSummary): MenuSection[] => {
    if (!conn || !hostState) return [];
    const key = projectKey(project);
    const glyph = (Glyph: ComponentType<{ size?: number; color?: string }>) => createElement(Glyph, { size: 18, color: t.text });
    const first: MenuItem[] = [];
    if (project.id !== '' && status.connected && hostState.hostMinor >= 7) {
      first.push({
        key: 'new',
        label: `New task in ${project.name}`,
        mark: glyph(Sparkle),
        onPress: () => open(`/new/${conn.id}?project=${encodeURIComponent(project.id)}`),
      });
    }
    if (project.id !== '' && hostState.hostMinor >= 9) {
      first.push({
        key: 'backlog',
        label: 'Backlog',
        mark: glyph(ListTodo),
        icon: project.backlog > 0 ? <Text style={styles.menuCount}>{project.backlog}</Text> : undefined,
        onPress: () => open(`/backlog/${conn.id}/${encodeURIComponent(project.id)}`),
      });
    }
    const pinned = pinnedHere(project);
    return [
      ...(first.length > 0 ? [{ items: first }] : []),
      {
        items: [
          {
            key: 'pin',
            label: pinned ? 'Unpin' : 'Pin to top',
            mark: glyph(pinned ? PinOff : Pin),
            onPress: () => keep(hiddenKeys, pinned ? pinnedKeys.filter((k) => k !== key) : [...pinnedKeys, key]),
          },
        ],
      },
      {
        items: [
          {
            key: 'hide',
            label: 'Hide on this phone',
            detail: 'Back in the list if an agent there needs you',
            mark: glyph(EyeOff),
            onPress: () => keep([...hiddenKeys, key], pinnedKeys),
          },
        ],
      },
    ];
  };
  const holdProject = (project: ProjectSummary) => {
    if (!conn) return;
    placeOf(held.current.get(`project:${projectKey(project)}`), (place) => {
      menuOpened();
      setProjectMenu({ project, place });
    });
  };
  // Shows a hidden project again; the sheet goes once none is left in it.
  const show = (keys: string[]) => {
    const hidden = hiddenKeys.filter((k) => !keys.includes(k));
    keep(hidden, pinnedKeys);
    if (!tucked.some((project) => hidden.includes(projectKey(project)))) setTucking(false);
  };

  const explainSecure = () =>
    Alert.alert(
      'End-to-end encrypted',
      `Everything between this phone and ${hostState?.host.name || 'your desktop'} is end-to-end encrypted. The relay only ever sees ciphertext.${status.kind ? `\n\n${status.text}.` : ''}`,
    );

  // The list's rows as the scroll view's own children, and which of them are
  // project headings: only a direct child can be pinned. Each section stands
  // 18 under the one above it; a label is 6 over its card.
  const listed = ((): { rows: ReactNode[]; pinned: number[] } => {
    const out: ReactNode[] = [];
    const pinned: number[] = [];
    const after = () => (out.length > 0 ? styles.section : null);
    if (conn && !filling && waiting.length > 0) {
      out.push(
        <SectionLabel key="waiting-label" color={t.attention}>
          Waiting on you
        </SectionLabel>,
      );
      out.push(
        <RuledCard key="waiting" style={styles.cardGap}>
          {waiting.map((w) => {
            const label = holdLabel(w);
            const agent = agentOf(w.worktree);
            return (
              <Row
                key={waitingKey(w)}
                ruled
                mark={
                  <AgentTile
                    agent={agent}
                    session={colorOf(w.worktree)}
                    tone={t.attention}
                    label={`${agent ? agentLabel(agent) : 'Terminal'}, waiting on you`}
                  />
                }
                title={w.worktree.name || 'Other terminals'}
                detail={w.question?.subject || detail(w.worktree)}
                detailColor={t.dim}
                meta={<Pill title="Answer" tone={t.attention} onPress={() => answer(w)} />}
                actions={label ? [{ name: 'answerWaiting', label, run: () => void approve(w) }] : undefined}
                onPress={() => answer(w)}
                onLongPress={label ? () => void approve(w) : undefined}
              />
            );
          })}
        </RuledCard>,
      );
    }
    if (filling) out.push(<ListSkeleton key="skeleton" />);
    for (const project of filling ? [] : projects) {
      const rows = project.worktrees.filter(
        (w) => !gone.includes(w.id) && (passes(w, filter) || (filter === 'unread' && stay.includes(w.id))),
      );
      // Work this phone just asked for, until its worktree is here.
      const starting =
        filter === 'all' || filter === 'active'
          ? (hostState?.starting ?? []).filter((work) => work.project === project.id)
          : [];
      // All lists every project, as the desktop's sidebar does —
      // a desktop from 1.9 sends them running or not. The other
      // filters list only projects with a row to show.
      if (rows.length === 0 && starting.length === 0 && filter !== 'all') continue;
      const key = projectKey(project);
      const folded = collapsed.includes(key);
      // Folded with nothing starting, the heading is all there is of it.
      const closed = folded && starting.length === 0;
      // Its heading, pinned, over the desk's own colour, so the rows pass
      // under it; then its card.
      pinned.push(out.length);
      out.push(
        <View key={`${key}:head`} style={[styles.headGround, after()]}>
          <View
            collapsable={false}
            ref={(view) => {
              if (view) held.current.set(`project:${key}`, view);
              else held.current.delete(`project:${key}`);
            }}
          >
            <ProjectHeading
              name={project.name || 'Elsewhere'}
              color={projectColor(project)}
              icon={project.icon}
              collapsed={folded}
              pinned={pinnedHere(project)}
              waiting={rows.some((w) => w.activity === Activity.BLOCKED)}
              onToggle={() => toggle(key)}
              onHold={() => holdProject(project)}
            />
          </View>
        </View>,
      );
      if (closed) continue;
      out.push(
        <RuledCard key={key} style={styles.cardGap}>
          {starting.map((work) => (
            <View key={`starting-${work.at}`}>
              <Row
                ruled
                mark={<AgentTile agent={work.agent || ''} tone={t.brand} label="Starting" />}
                title={firstLine(work.prompt)}
                detail={`${stageText(progressOf(work, hostState?.snapshot ?? null), hostState?.host.name || 'your desktop', work.agent)} · ${elapsed(work.at, now)}`}
                onPress={() => open(`/starting/${conn?.id ?? ''}/${work.at}`)}
              />
              <View style={styles.startingTrack}>
                <Track state="live" />
              </View>
            </View>
          ))}
          {!folded && rows.length === 0 && starting.length === 0 ? (
            <Prose key="idle" style={styles.idle}>
              Nothing running
            </Prose>
          ) : null}
          {folded
            ? null
            : rows.map((worktree) => (
                <View
                  key={worktree.id}
                  collapsable={false}
                  // Read since it was opened from here: faded, until Unread is left.
                  style={filter === 'unread' && !isUnread(worktree) ? styles.read : undefined}
                  ref={(view) => {
                    if (view) held.current.set(worktree.id, view);
                    else held.current.delete(worktree.id);
                  }}
                >
                  <WorktreeRow
                    worktree={worktree}
                    waking={waking.includes(worktree.id)}
                    marker={worktree.id === opened}
                    onPress={() => follow(worktree)}
                    onLongPress={() => hold(worktree)}
                  />
                </View>
              ))}
        </RuledCard>,
      );
    }
    if (empty && conn) out.push(<NothingRunning key="nothing" host={hostState.host.name} />);
    if (!filling && filter !== 'all' && all.length > 0 && count(filter) === 0 && stay.length === 0) {
      out.push(
        <Prose key="note" style={[styles.note, after()]}>
          {EMPTY[filter]}
        </Prose>,
      );
    }
    if (!filling && tuckedAway > 0) {
      const gap = after();
      out.push(
        <Pressable
          key="tucked"
          accessibilityRole="button"
          onPress={() => setTucking(true)}
          style={({ pressed }) => [styles.tucked, gap, pressed && { backgroundColor: t.hover }]}
        >
          <EyeOff size={14} color={t.faint} />
          <Text style={styles.tuckedText}>{tuckedAway} hidden on this phone</Text>
        </Pressable>,
      );
    }
    return { rows: out, pinned };
  })();

  return (
    <Screen>
      {/* The desktop is the screen's large title; a tap opens the Mac
          switcher. Under it, the lock on the line to it and how it runs —
          a tap says what that means. Across from them, Start. */}
      <View style={styles.head}>
        <View style={styles.headText}>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Switch desktop"
            onPress={() => setPicking(true)}
            hitSlop={6}
            style={({ pressed }) => [styles.titleRow, pressed && styles.pressed]}
          >
            <Text style={styles.titleText} numberOfLines={1}>
              {hostState?.host.name ?? ''}
            </Text>
            <ChevronDown size={14} color={t.faint} weight={2.4} />
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${status.connected ? 'Secure: end-to-end encrypted' : statusWords}${status.kind ? `, ${status.text}` : ''}`}
            onPress={explainSecure}
            hitSlop={6}
            style={({ pressed }) => [styles.status, pressed && styles.pressed]}
          >
            <Seal sealed={status.connected} size={11} />
            <Text style={styles.statusText} numberOfLines={1}>
              {statusWords}
            </Text>
          </Pressable>
        </View>
        {conn && status.connected ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Start a task"
            onPress={() => open(`/new/${conn.id}`)}
            style={({ pressed }) => [styles.new, pressed && { backgroundColor: '#9aeaa4' }]}
          >
            <Sparkle size={15} color={t.onBrand} weight={2.2} />
            <Text style={styles.newText}>Start</Text>
          </Pressable>
        ) : null}
      </View>

      {conn && down ? (
        <ScrollView contentContainerStyle={styles.downPage}>
          <Connecting conn={conn} />
        </ScrollView>
      ) : (
        <>
          {/* What the list shows: an icon each, in its state's colour. */}
          <Tabs
            style={styles.tabs}
            value={filter}
            onChange={pickFilter}
            tabs={[
              {
                key: 'all' as Filter,
                label: 'All',
                name: named('All', 'all'),
                count: tally('all'),
                icon: (on) => <Wildcard size={15} color={on ? t.text : t.dim} weight={2.4} />,
              },
              {
                key: 'active' as Filter,
                label: 'Active',
                tone: t.brand,
                name: named('Active', 'active'),
                count: tally('active'),
                icon: (on) => <Bolt size={15} color={t.brand} weight={1.9} fill={on ? tint(t.brand, 0.25) : undefined} />,
              },
              // A conversation wearing the row's own unread dot.
              {
                key: 'unread' as Filter,
                label: 'Unread',
                name: named('Unread', 'unread'),
                count: tally('unread'),
                icon: (on) => (
                  <Bubble size={15} color={t.text} weight={1.9} dot={t.backdrop} fill={on ? tint(t.text, 0.18) : undefined} />
                ),
              },
              {
                key: 'ready' as Filter,
                label: 'Ready',
                tone: t.merging,
                name: named('Ready', 'ready'),
                count: tally('ready'),
                icon: () => <Merge size={15} color={t.merging} weight={1.9} />,
              },
            ]}
          />

          <Animated.ScrollView
            // Mounted anew as the list lands, so it fades in over its skeleton.
            key={filling ? 'filling' : 'filled'}
            entering={filling ? undefined : FADE_IN}
            scrollEnabled={!filling}
            keyboardDismissMode="on-drag"
            contentContainerStyle={[styles.list, { paddingBottom: under }, empty && styles.listEmpty]}
            // A project's heading stays at the top while its rows pass under it.
            stickyHeaderIndices={listed.pinned}
          >
            {listed.rows}
          </Animated.ScrollView>
          <FootFade height={under} />
        </>
      )}

      <TabBar active={sidebar ? tabAt(path) : 'worktrees'} needs={waitingEverywhere} sidebar={sidebar} />

      <FloatMenu
        open={menu !== null}
        onClose={() => setMenu(null)}
        anchor={menu?.place ?? null}
        label={menu ? `${menu.worktree.name} actions` : 'Worktree actions'}
        lifted={
          menu ? (
            // On its card's colour, as it was in the list: its tile is the
            // raised grey the lifted ground is.
            <View style={styles.liftedRow}>
              <WorktreeRow worktree={menu.worktree} />
            </View>
          ) : null
        }
        sections={sections}
      />
      <FloatMenu
        open={projectMenu !== null}
        onClose={() => setProjectMenu(null)}
        anchor={projectMenu?.place ?? null}
        label={projectMenu ? `${projectMenu.project.name || 'Elsewhere'} actions` : 'Project actions'}
        lifted={
          projectMenu ? (
            <ProjectHeading
              name={projectMenu.project.name || 'Elsewhere'}
              color={projectColor(projectMenu.project)}
              icon={projectMenu.project.icon}
              collapsed
              pinned={pinnedHere(projectMenu.project)}
            />
          ) : null
        }
        sections={projectMenu ? projectSections(projectMenu.project) : []}
      />
      {/* The projects hidden on this phone, each to show again. */}
      <Sheet
        open={tucking}
        onClose={() => setTucking(false)}
        title="Hidden"
        detail={`On this phone only. They stay in ket on ${hostState?.host.name || 'your Mac'}, and Start still offers them.`}
      >
        <SheetList>
          {tucked.map((project) => {
            const asking = project.worktrees.filter((w) => w.activity === Activity.BLOCKED).length;
            return (
              <SheetItem
                key={projectKey(project)}
                label={project.name || 'Elsewhere'}
                mark={<Badge name={project.name || 'Elsewhere'} color={projectColor(project)} icon={project.icon} />}
                detail={
                  asking > 0
                    ? `${asking} waiting · in the list until answered`
                    : project.worktrees.map((w) => w.name || 'Other terminals').join(' · ') || 'No worktrees open'
                }
                icon={
                  <View style={styles.show}>
                    <Text style={styles.showText}>Show</Text>
                  </View>
                }
                onPress={() => show([projectKey(project)])}
              />
            );
          })}
        </SheetList>
        <Button title={`Show all ${tucked.length}`} onPress={() => show(tucked.map(projectKey))} />
      </Sheet>
      {/* Every paired Mac, and a way to add one — offered with a single Mac
          too, so the switcher says it can hold more. Adding one is not wired
          yet. */}
      <Sheet
        open={picking}
        onClose={() => setPicking(false)}
        onGone={() => {
          if (!toAutomations) return;
          setToAutomations(false);
          setAutomating(true);
        }}
        title="Macs"
        detail="Paired with this device"
      >
        <SheetList>
          {(hosts ?? []).map(({ conn: c, hostState: h }) => (
            <SheetItem
              key={c.id}
              role="radio"
              label={h.host.name}
              mono
              mark={<Desktop size={18} color={t.text} />}
              detail={statusLine(h).text}
              dot={h.status === 'connected' ? t.running : h.status === 'offline' ? t.failed : t.faint}
              on={c === conn}
              onPress={() => {
                setPicking(false);
                choose(c);
              }}
            />
          ))}
        </SheetList>
        {conn && hostState && hostState.hostMinor >= 36 ? (
          <SheetList>
            <SheetItem
              label="Automations"
              mark={<Clock size={18} color={t.text} />}
              detail={`Work that starts itself on ${hostState.host.name}: pause one, or start it now.`}
              icon={<Chevron size={12} color={t.dim} weight={2.4} />}
              onPress={() => {
                setToAutomations(true);
                setPicking(false);
              }}
            />
          </SheetList>
        ) : null}
        <SheetList>
          <SheetItem
            label="Add another Mac"
            mark={
              <View style={styles.addMark}>
                <Plus size={10} color={t.text} weight={3} />
              </View>
            }
            detail="Pair ket on a second Mac, then switch between their agents here."
            icon={<Chevron size={12} color={t.dim} weight={2.4} />}
            onPress={() => {}}
          />
        </SheetList>
        <SheetNote>On that Mac: Settings › Devices › Pair a device, then scan its code here.</SheetNote>
      </Sheet>
      <AutomationsSheet
        open={automating}
        onClose={() => setAutomating(false)}
        conn={conn ?? null}
        hostName={hostState?.host.name ?? 'your Mac'}
      />
    </Screen>
  );
}

/** The desk darkening from where the last card comes to rest down under the
 * floating tab bar, so what scrolls beneath the bar fades out rather than
 * meeting it, and nothing at rest is dimmed. */
function FootFade({ height }: { height: number }) {
  return (
    <View pointerEvents="none" style={[styles.fade, { height }]}>
      <Svg width="100%" height="100%">
        <Defs>
          <LinearGradient id="list-fade" x1="0" y1="0" x2="0" y2="1">
            <Stop offset="0" stopColor={t.backdrop} stopOpacity={0} />
            <Stop offset="0.55" stopColor={t.backdrop} stopOpacity={0.88} />
            <Stop offset="1" stopColor={t.backdrop} stopOpacity={1} />
          </LinearGradient>
        </Defs>
        <Rect width="100%" height="100%" fill="url(#list-fade)" />
      </Svg>
    </View>
  );
}

/** The list once it has settled with nothing on the desktop at all: a
 * quiet mark in a well, centred in the sheet, and what would fill it. */
function NothingRunning({ host }: { host: string }) {
  return (
    <View style={styles.empty} accessibilityLiveRegion="polite">
      <View style={styles.emptyMark}>
        <Desktop size={20} color={t.faint} />
      </View>
      <Text style={styles.emptyTitle}>Nothing running</Text>
      <Prose style={styles.emptyNote}>Agents and terminals you start on {host || 'your Mac'} show up here.</Prose>
    </View>
  );
}

const styles = StyleSheet.create({
  // The title and the line under it, and Start across from them.
  head: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
    paddingTop: 10,
    paddingHorizontal: EDGE,
  },
  headText: { flexShrink: 1, minWidth: 0, gap: 3 },
  titleRow: { alignSelf: 'flex-start', maxWidth: '100%', flexDirection: 'row', alignItems: 'center', gap: 7 },
  titleText: { flexShrink: 1, fontFamily: font.proseSemibold, fontSize: 22, lineHeight: 28, letterSpacing: -0.3, color: t.text },
  status: { alignSelf: 'flex-start', flexDirection: 'row', alignItems: 'center', gap: 5 },
  statusText: { fontFamily: font.chrome, fontSize: size.label, lineHeight: 18, color: t.faint },
  pressed: { opacity: 0.6 },
  tabs: { marginTop: 14 },
  // The empty slot a Mac would fill: a dashed square with a plus in it.
  addMark: {
    width: 18,
    height: 18,
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t.faint,
    alignItems: 'center',
    justifyContent: 'center',
  },
  downPage: { flexGrow: 1, padding: size.gutter, paddingTop: 4 },
  // The one way to start work from here, so it wears ket's green — the
  // colour Send fills with when there is something to send.
  new: {
    height: 40,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingLeft: 13,
    paddingRight: 16,
    borderRadius: 20,
    backgroundColor: t.brand,
  },
  // A row read since it was opened from under Unread.
  read: { opacity: 0.55 },
  newText: { fontFamily: font.proseSemibold, fontSize: size.label, color: t.onBrand },
  // Under the tab bar, it pads the foot — see `under`.
  list: { paddingHorizontal: EDGE, paddingTop: 16 },
  // A section under the one above it: a project, what waits on you.
  section: { marginTop: 18 },
  // A card under its label.
  cardGap: { marginTop: 6 },
  // A heading's ground: the desk's, so the rows pass under it.
  headGround: { backgroundColor: t.backdrop },
  liftedRow: { backgroundColor: t.card },
  fade: { position: 'absolute', left: 0, right: 0, bottom: 0 },
  // Under the cards: the line to the projects hidden on this phone.
  tucked: {
    height: size.control,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
    borderRadius: size.radiusRow,
  },
  tuckedText: { fontFamily: font.chrome, fontSize: 12, color: t.faint },
  show: { paddingHorizontal: 12, paddingVertical: 6, borderRadius: size.radius, backgroundColor: t.selection },
  showText: { fontFamily: font.proseMedium, fontSize: size.label, color: t.text },
  menuCount: { fontFamily: font.chrome, fontSize: 12, color: t.faint },
  // Nothing on the desktop: the list grows to the screen, to centre the note.
  listEmpty: { flexGrow: 1 },
  empty: { flexGrow: 1, alignItems: 'center', justifyContent: 'center', gap: 6, paddingHorizontal: 28, paddingBottom: 40 },
  emptyMark: {
    width: 44,
    height: 44,
    marginBottom: 8,
    borderRadius: size.radiusLg,
    alignItems: 'center',
    justifyContent: 'center',
    ...surface.well,
  },
  emptyTitle: { fontFamily: font.proseSemibold, fontSize: size.title, color: t.text, textAlign: 'center' },
  emptyNote: { textAlign: 'center' },
  note: { padding: 8 },
  // A project's card with nothing in it, in from the edge its rows start at.
  idle: { fontSize: size.label, paddingHorizontal: 14, paddingVertical: 12 },
  // Under a row standing in for new work: the line that says it is moving.
  startingTrack: { paddingHorizontal: 14, paddingBottom: 8 },
});
