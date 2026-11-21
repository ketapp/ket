// Worktrees: the desktop sidebar, on a phone. Along the top, ket's mark and
// whether the line is encrypted, over the desktop's name as a large title
// and how it is reached; under it the sidebar's filter and Start, which
// opens the New task sheet, then what waits on you and a card for each
// project, its worktrees in it, on the desk. The cards scroll under the tab
// bar, which floats over the desk's foot, and come to rest above it.
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

import { Activity, type ProjectSummary, type WorktreeSummary } from '@ket/remote';
import { router, useFocusEffect, useGlobalSearchParams, usePathname } from 'expo-router';
import { type ComponentType, type ReactNode, createElement, useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Pressable, ScrollView, StyleSheet, Text, View, type ViewInstance } from 'react-native';
import Animated, { FadeIn, ReduceMotion } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { Connecting } from './Connecting';
import { ListSkeleton } from './ListSkeleton';
import { Track } from './Track';
import { FloatMenu, type MenuItem, type MenuSection, type Place, placeOf } from './FloatMenu';
import { Seal } from './Seal';
import { Sheet, SheetItem, SheetList, SheetNote } from './Sheet';
import { Unpaired } from './Unpaired';
import {
  AgentMark,
  BrandMark,
  Chevron,
  ChevronDown,
  Desktop,
  EyeOff,
  Hand,
  ListTodo,
  Merge,
  Pin,
  PinOff,
  Plus,
  Sparkle,
  ADDRESS_ICON,
} from './icons';
import {
  Badge,
  Button,
  Dot,
  ProjectHeading,
  Prose,
  Row,
  MARK_GAP,
  MARK_SLOT,
  ROW_INSET,
  Screen,
  Segmented,
  TAB_BAR_HEIGHT,
  TabBar,
  tabBarFoot,
} from './ui';
import {
  type Filter,
  agentTerminal,
  asleepTab,
  detail,
  everyWorktree,
  isUnread,
  passes,
  projectColor,
  projectKey,
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
import { font, size, t, tint } from '../lib/theme';
import { worktreeMenu } from './WorktreeMenu';
import { WorktreeRow } from './WorktreeRow';

/** Between the last card, at rest, and the tab bar under it. */
const FOOT_GAP = 10;

/** The filter track and Start beside it, a capsule each. */
const FILTER_HEIGHT = 41;

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
  const waiting = located.filter((l) => l.worktree.activity === Activity.BLOCKED);
  const status = statusLine(hostState);
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

  const answer = () =>
    waiting.length === 1 && conn
      ? open(
          (waiting[0].worktree.questions[0] ?? waiting[0].worktree.question)?.terminalId
            ? `/session/${conn.id}/${(waiting[0].worktree.questions[0] ?? waiting[0].worktree.question)!.terminalId.toString()}`
            : `/worktree/${conn.id}/${encodeURIComponent(waiting[0].worktree.id)}`,
        )
      : sidebar
        ? showBeside('/needs')
        : router.replace('/needs');

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
  // project headings: only a direct child can be pinned.
  const listed = ((): { rows: ReactNode[]; pinned: number[] } => {
    const out: ReactNode[] = [];
    const pinned: number[] = [];
    const add = (row: ReactNode) => {
      if (row) out.push(row);
    };
    add(
      waiting.length > 0 && conn && !filling ? (
        <Pressable key="waiting" style={({ pressed }) => [styles.alertRow, pressed && { backgroundColor: tint(t.attention, 0.14) }]} onPress={answer}>
          <View style={styles.mark}>
            <Hand size={17} color={t.attention} />
          </View>
          <View style={styles.alertText}>
            <Text style={styles.bannerTitle}>{waiting.length === 1 ? '1 waiting on you' : `${waiting.length} waiting on you`}</Text>
            <Text style={styles.bannerDetail} numberOfLines={1}>
              {waiting[0].worktree.name} ·{' '}
              {(waiting[0].worktree.questions[0] ?? waiting[0].worktree.question)?.subject || detail(waiting[0].worktree)}
            </Text>
          </View>
          <View style={styles.answerLink}>
            <Text style={styles.answerLinkText}>Answer</Text>
            <Chevron size={13} color={t.attention} />
          </View>
        </Pressable>
      ) : null,
    );
    add(
      filling ? (
        <View key="skeleton" style={styles.card}>
          <ListSkeleton />
        </View>
      ) : null,
    );
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
      // A desktop from 1.9 keeps its backlogs.
      const keeps = hostState !== null && hostState.hostMinor >= 9 && project.id !== '';
      const key = projectKey(project);
      const folded = collapsed.includes(key);
      // Folded with nothing starting, the heading is the whole card: a body
      // with no rows is 5 pt tall, too short for its 12 pt corners, which
      // the platform shrinks to fit, so they would not match the top's.
      const closed = folded && starting.length === 0;
      // The card in two: its heading, pinned, over the page's own colour so
      // nothing shows round its corners; and its rows.
      pinned.push(out.length);
      out.push(
        <View key={`${key}:head`} style={styles.cardHeadGround}>
          <View style={[styles.cardHead, closed && styles.cardHeadClosed]}>
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
                inside={rows.map((w) => w.name || 'Other terminals').join(' · ')}
                pinned={pinnedHere(project)}
                waiting={rows.some((w) => w.activity === Activity.BLOCKED)}
                onToggle={() => toggle(key)}
                onHold={() => holdProject(project)}
                backlog={project.backlog}
                onBacklog={
                  keeps && conn
                    ? () => open(`/backlog/${conn.id}/${encodeURIComponent(project.id)}`)
                    : undefined
                }
              />
            </View>
          </View>
        </View>,
      );
      if (closed) continue;
      out.push(
        <View key={key} style={styles.cardBody}>
          {starting.map((work) => (
            <View key={`starting-${work.at}`}>
              <Row
                mark={work.agent ? <AgentMark agent={work.agent} size={16} well /> : null}
                title={firstLine(work.prompt)}
                detail={`${stageText(progressOf(work, hostState?.snapshot ?? null), hostState?.host.name || 'your desktop', work.agent)} · ${elapsed(work.at, now)}`}
                state={{ word: 'starting', color: t.brand }}
                chevron
                onPress={() => open(`/starting/${conn?.id ?? ''}/${work.at}`)}
              />
              <View style={styles.startingTrack}>
                <Track state="live" />
              </View>
            </View>
          ))}
          {!folded && rows.length === 0 && starting.length === 0 ? (
            <Prose style={styles.idle}>Nothing running</Prose>
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
        </View>,
      );
    }
    add(empty && conn ? <NothingRunning key="nothing" host={hostState.host.name} /> : null);
    add(
      !filling && filter !== 'all' && all.length > 0 && count(filter) === 0 && stay.length === 0 ? (
        <Prose key="note" style={styles.note}>
          {EMPTY[filter]}
        </Prose>
      ) : null,
    );
    add(
      !filling && tuckedAway > 0 ? (
        <Pressable
          key="tucked"
          accessibilityRole="button"
          onPress={() => setTucking(true)}
          style={({ pressed }) => [styles.tucked, pressed && { backgroundColor: t.selection }]}
        >
          <EyeOff size={14} color={t.faint} />
          <Text style={styles.tuckedText}>{tuckedAway} hidden on this phone</Text>
        </Pressable>
      ) : null,
    );
    return { rows: out, pinned };
  })();

  return (
    <Screen>
      {/* ket's mark, where Back sits on every other screen, wears the line
          to the desktop: its green when up, grey while reaching, red when
          down. Across from it, the lock on that line, what it says, and
          after a hairline a mark for how it runs: Wi-Fi, VPN or a cloud. */}
      <View style={styles.top}>
        <View style={styles.brand}>
          <BrandMark size={17} color={status.connected ? t.brand : hostState?.status === 'offline' ? t.failed : t.faint} />
        </View>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`Secure: end-to-end encrypted${status.kind ? `, ${status.text}` : ''}`}
          onPress={explainSecure}
          hitSlop={6}
          style={({ pressed }) => [
            styles.secure,
            { backgroundColor: status.connected ? tint(t.running, pressed ? 0.22 : 0.13) : pressed ? t.border : t.selection },
          ]}
        >
          <Seal sealed={status.connected} size={13} />
          <Text style={[styles.secureText, { color: status.connected ? t.running : t.dim }]}>
            {status.connected ? 'Secure' : hostState?.status === 'offline' ? 'Offline' : 'Reaching'}
          </Text>
          {status.kind ? (
            <>
              <View style={styles.secureRule} />
              {createElement(ADDRESS_ICON[status.kind], { size: 14, color: t.running, weight: 2.2 })}
            </>
          ) : null}
        </Pressable>
      </View>
      {/* The desktop is the screen's large title; a tap opens the Mac
          switcher. */}
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Switch desktop"
        onPress={() => setPicking(true)}
        style={({ pressed }) => [styles.title, (pressed || picking) && { backgroundColor: t.selection }]}
      >
        <View style={styles.titleRow}>
          <Text style={styles.titleText} numberOfLines={1}>
            {hostState?.host.name ?? ''}
          </Text>
          <ChevronDown size={14} color={t.dim} weight={2.4} />
        </View>
      </Pressable>

      {conn && down ? (
        <ScrollView contentContainerStyle={styles.downPage}>
          <Connecting conn={conn} />
        </ScrollView>
      ) : (
        <>
          <View style={styles.filters}>
            <Segmented
              style={styles.track}
              round
              height={FILTER_HEIGHT - 6}
              value={filter}
              onChange={pickFilter}
              segments={[
                { key: 'all' as Filter, label: 'All', count: tally('all') },
                { key: 'active' as Filter, label: '', name: named('Active', 'active'), icon: <Dot color={t.running} size={7} />, count: tally('active') },
                // The row's own unread mark.
                { key: 'unread' as Filter, label: '', name: named('Unread', 'unread'), icon: <Dot color={t.accent} size={7} />, count: tally('unread') },
                { key: 'ready' as Filter, label: '', name: named('Ready', 'ready'), icon: <Merge size={12} color={t.merging} weight={2.4} />, count: tally('ready') },
              ]}
            />
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
        lifted={menu ? <WorktreeRow worktree={menu.worktree} /> : null}
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
              inside={projectMenu.project.worktrees.map((w) => w.name || 'Other terminals').join(' · ')}
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
      <Sheet open={picking} onClose={() => setPicking(false)} title="Macs" detail="Paired with this device">
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
  // ket's mark and the lock's pill, over the title.
  top: { height: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: size.gutter },
  // Back's slot on the other screens: 40 square, in 6 from the gutter.
  brand: { width: 40, height: 40, marginLeft: -6, alignItems: 'center', justifyContent: 'center' },
  secure: { height: 37, flexDirection: 'row', alignItems: 'center', gap: 6, paddingLeft: 11, paddingRight: 13, borderRadius: 18.5 },
  secureText: { fontFamily: font.chrome, fontSize: 11.5 },
  secureRule: { width: StyleSheet.hairlineWidth * 2, height: 14, marginHorizontal: 2, backgroundColor: tint(t.running, 0.35) },
  // The desktop's name as a large title, in from the gutter as a card's
  // heading is, so it lines up with the list under it.
  title: { marginHorizontal: size.gutter, marginTop: 2, paddingHorizontal: 6, paddingVertical: 2, gap: 4, borderRadius: size.radiusLg },
  titleRow: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  titleText: { flexShrink: 1, fontFamily: font.proseSemibold, fontSize: 22, lineHeight: 28, letterSpacing: -0.3, color: t.text },
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
  // The filter and Start, on the desk over the cards.
  filters: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingHorizontal: size.gutter, paddingTop: 6, paddingBottom: 8 },
  track: { flex: 1 },
  // A project: a card on the desk, its heading and its worktrees in it.
  card: {
    padding: 4,
    backgroundColor: t.card,
    borderWidth: 1,
    borderColor: t.rule,
    borderRadius: size.radiusCard,
  },
  // A project's card in two pieces, so its heading can stay at the top of
  // the list: the heading's half, and under it the rows', pulled up over the
  // list's gap to meet it.
  cardHeadGround: { backgroundColor: t.backdrop },
  cardHead: {
    padding: 4,
    paddingBottom: 0,
    backgroundColor: t.card,
    borderWidth: 1,
    borderBottomWidth: 0,
    borderColor: t.rule,
    borderTopLeftRadius: size.radiusCard,
    borderTopRightRadius: size.radiusCard,
  },
  // A folded card's heading, closing the card where its body would: the
  // body's 4 pt foot and its border, so the card stands as tall as before.
  cardHeadClosed: {
    paddingBottom: 4,
    borderBottomWidth: 1,
    borderBottomLeftRadius: size.radiusCard,
    borderBottomRightRadius: size.radiusCard,
  },
  cardBody: {
    marginTop: -6,
    padding: 4,
    paddingTop: 0,
    backgroundColor: t.card,
    borderWidth: 1,
    borderTopWidth: 0,
    borderColor: t.rule,
    borderBottomLeftRadius: size.radiusCard,
    borderBottomRightRadius: size.radiusCard,
  },
  // The one way to start work from here, so it wears ket's green — the
  // colour Send fills with when there is something to send.
  new: {
    height: FILTER_HEIGHT,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingLeft: 14,
    paddingRight: 16,
    borderRadius: FILTER_HEIGHT / 2,
    backgroundColor: t.brand,
  },
  // A row read since it was opened from under Unread.
  read: { opacity: 0.55 },
  newText: { fontFamily: font.proseSemibold, fontSize: size.label, color: t.onBrand },
  // What waits on you: a card of its own, over the projects. Its hand
  // stands in the column the badges and marks below share — in by a
  // card's padding and a row's inset — so its words start where theirs do.
  alertRow: {
    borderRadius: size.radiusCard,
    borderWidth: 1,
    borderColor: tint(t.attention, 0.16),
    paddingVertical: 11,
    paddingLeft: 4 + ROW_INSET,
    paddingRight: 11,
    flexDirection: 'row',
    alignItems: 'center',
    gap: MARK_GAP,
    backgroundColor: tint(t.attention, 0.08),
  },
  alertText: { flex: 1, minWidth: 0, gap: 2 },
  bannerTitle: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  bannerDetail: { fontFamily: font.chrome, fontSize: 12, color: t.dim },
  answerLink: { flexDirection: 'row', alignItems: 'center', gap: 2 },
  answerLinkText: { fontFamily: font.proseMedium, fontSize: size.label, color: t.attention },
  // The cards, 6 apart; under the tab bar, it pads the foot — see `under`.
  list: { paddingHorizontal: size.gutter, gap: 6 },
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
    backgroundColor: t.sunken,
  },
  emptyTitle: { fontFamily: font.proseSemibold, fontSize: size.title, color: t.text, textAlign: 'center' },
  emptyNote: { textAlign: 'center' },
  mark: { width: MARK_SLOT, alignItems: 'center' },
  note: { padding: 8 },
  // Under a project's heading, in from the edge its rows start at.
  idle: { fontSize: size.detail, paddingLeft: 14, paddingBottom: 8 },
  // Under a row standing in for new work: the line that says it is moving.
  startingTrack: { paddingHorizontal: 14, paddingBottom: 8 },
});
