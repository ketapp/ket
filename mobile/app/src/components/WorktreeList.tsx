'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

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

import {
  Activity,
  type ProjectSummary,
  type SubagentSummary,
  SubagentSummary_State,
  type WorktreeSummary,
} from '@ket/remote';
import { router, useFocusEffect, useGlobalSearchParams, usePathname } from 'expo-router';
import { type ComponentType, createElement, useCallback, useEffect, useRef, useState } from 'react';
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
import { Fade, Wipe } from './Swap';
import { Unpaired } from './Unpaired';
import {
  AgentMark,
  BrandMark,
  Check,
  CheckCircle,
  Chevron,
  ChevronDown,
  Comment,
  Desktop,
  EyeOff,
  Hand,
  ListTodo,
  Merge,
  Pin,
  PinOff,
  Plus,
  PullRequestGlyph,
  Refresh,
  Sparkle,
  Trash,
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
  ASLEEP,
  type Filter,
  WAKING,
  agentOf,
  agentTerminal,
  asleepTab,
  detail,
  detailColor,
  everyWorktree,
  filterOf,
  projectColor,
  projectKey,
  stateOf,
} from '../lib/activity';
import { elapsed, firstLine, progressOf, stageText, startingAt } from '../lib/starting';
import { checksColor, checksState } from '../lib/checks';
import type { HostConnection } from '../lib/connection';
import { menuOpened } from '../lib/haptics';
import { statusLine, useHosts } from '../lib/hosts';
import { showBeside, tabAt, worktreeAt } from '../lib/layout';
import { confirmIdentity } from '../lib/lock';
import { askMerge, prActions } from '../lib/pr-actions';
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

/** Between the last card, at rest, and the tab bar under it. */
const FOOT_GAP = 10;

/** The list coming in over its skeleton. */
const FADE_IN = FadeIn.duration(220).reduceMotion(ReduceMotion.System);

/** The worktree list. `sidebar` draws it as the wide screen's sidebar: the
 * open worktree marked, and every row opening beside it. `refresh` re-reads
 * the paired desktops and the chosen one when it changes — the sidebar is
 * never focused anew the way a tab is, so the layout passes the path. */
export function WorktreeList({ sidebar = false, refresh }: { sidebar?: boolean; refresh?: string }) {
  const conns = useHosts(refresh);
  const path = usePathname();
  // A starting screen that has become its terminal's chat names it here.
  const { terminal: shownTerminal } = useGlobalSearchParams<{ terminal?: string }>();
  const insets = useSafeAreaInsets();
  const open = (href: string) => (sidebar ? showBeside(href) : router.push(href));
  const [chosen, setChosen] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
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

  const conn = conns?.find((c) => c.host.id === chosen) ?? conns?.[0] ?? null;
  // Work this phone asked for counts the seconds it has waited, in its row.
  const anyStarting = (conn?.starting.length ?? 0) > 0;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!anyStarting) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [anyStarting]);
  const hostId = conn?.host.id ?? null;
  // The list while it fills: from the desktop's welcome until its list is
  // here, and a moment longer when that list is empty — see lib/settling.
  // `now` can be older than the welcome; the clock is read again when the
  // wait ends, or at once if it already has, to show what is there.
  const filling = conn !== null && settling(conn, now);
  const until = filling && conn ? settlesAt(conn) : null;
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
  const snapshot = conn?.snapshot ?? null;
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

  const waitingEverywhere = (conns ?? []).reduce(
    (n, c) => n + everyWorktree(c.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );

  if (conns !== null && conns.length === 0) {
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
  const projects = (conn?.snapshot?.projects ?? [])
    .filter((project) => !hiddenHere(project) || waitsOnYou(project))
    .sort((a, b) => Number(pinnedHere(b)) - Number(pinnedHere(a)));
  const tucked = (conn?.snapshot?.projects ?? []).filter(hiddenHere);
  const tuckedAway = tucked.filter((project) => !waitsOnYou(project)).length;
  const located = projects.flatMap((project) => project.worktrees.map((worktree) => ({ project, worktree })));
  const all = located.map((l) => l.worktree);
  const count = (f: Filter) => all.filter((w) => f === 'all' || filterOf(w.activity) === f).length;
  // No counts while the list fills: a row of zeros would say nothing runs.
  const tally = (f: Filter) => (filling ? undefined : count(f));
  const named = (word: string, f: Filter) => (filling ? word : `${word}, ${count(f)}`);
  // Nothing at all on the desktop, now that its list has settled.
  const empty = !filling && conn?.status === 'connected' && conn.snapshot !== null && conn.snapshot.projects.length === 0;
  const waiting = located.filter((l) => l.worktree.activity === Activity.BLOCKED);
  const status = statusLine(conn);
  // Reaching for the desktop: its worktrees are not known yet, so the screen
  // shows the reaching instead of an empty list.
  const down = conn !== null && conn.status !== 'connected';
  // What the last card comes to rest above: the tab bar floating over the
  // desk, and a gutter between the two.
  const under = tabBarFoot(insets.bottom) + TAB_BAR_HEIGHT + FOOT_GAP;

  // The worktree open beside the sidebar.
  const opened =
    sidebar && conn
      ? (worktreeAt(path, conn.host.id, conn.snapshot) ??
        startingAt(path, conn.host.id, conn.asked, conn.snapshot, shownTerminal))
      : null;

  const choose = (c: HostConnection) => {
    setChosen(c.host.id);
    void saveDesktop(c.host.id);
  };

  const answer = () =>
    waiting.length === 1 && conn
      ? open(
          (waiting[0].worktree.questions[0] ?? waiting[0].worktree.question)?.terminalId
            ? `/session/${conn.host.id}/${(waiting[0].worktree.questions[0] ?? waiting[0].worktree.question)!.terminalId.toString()}`
            : `/worktree/${conn.host.id}/${encodeURIComponent(waiting[0].worktree.id)}`,
        )
      : sidebar
        ? showBeside('/needs')
        : router.replace('/needs');

  // Straight into the work: the agent's conversation, or with no agent, the
  // first terminal still open.
  const follow = (worktree: WorktreeSummary) => {
    if (!conn) return;
    const agentTerm = agentTerminal(worktree);
    if (agentTerm) {
      open(`/session/${conn.host.id}/${agentTerm.id.toString()}`);
      return;
    }
    // Claude's agent view in a plain shell tab — `claude agents` typed into
    // it — has no agent of its own; its sessions list is still the place.
    const view = worktree.terminals.find((x) => !x.closed && x.agentView);
    if (view) {
      open(`/session/${conn.host.id}/${view.id.toString()}`);
      return;
    }
    const running = worktree.terminals.find((x) => !x.closed);
    if (running) {
      open(`/terminal/${conn.host.id}/${running.id.toString()}`);
      return;
    }
    // An agent tab the desktop put to sleep: woken there, then its chat —
    // before a terminal that has exited, which has nothing left to show.
    if (conn.hostMinor >= 23 && worktree.asleep.length > 0) {
      void wake(worktree);
      return;
    }
    const exited = worktree.terminals[0];
    if (exited) {
      open(`/terminal/${conn.host.id}/${exited.id.toString()}`);
      return;
    }
    // No terminal at all — work Claude's agent view runs there under its own
    // daemon has none: the worktree's own screen, rather than a tap that does
    // nothing.
    open(`/worktree/${conn.host.id}/${encodeURIComponent(worktree.id)}`);
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
    open(`/session/${conn.host.id}/${woke.terminalId.toString()}`);
  };

  const going = (id: string, away: boolean) =>
    setGone((was) => (away ? (was.includes(id) ? was : [...was, id]) : was.filter((x) => x !== id)));
  const sections = menu && conn ? worktreeMenu(conn, menu.worktree, open, going) : [];
  const hold = (worktree: WorktreeSummary) => {
    if (!conn || worktreeMenu(conn, worktree, open, going).length === 0) return;
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
    if (!conn) return [];
    const key = projectKey(project);
    const glyph = (Glyph: ComponentType<{ size?: number; color?: string }>) => createElement(Glyph, { size: 18, color: t.text });
    const first: MenuItem[] = [];
    if (project.id !== '' && status.connected && conn.hostMinor >= 7) {
      first.push({
        key: 'new',
        label: `New task in ${project.name}`,
        mark: glyph(Sparkle),
        onPress: () => open(`/new/${conn.host.id}?project=${encodeURIComponent(project.id)}`),
      });
    }
    if (project.id !== '' && conn.hostMinor >= 9) {
      first.push({
        key: 'backlog',
        label: 'Backlog',
        mark: glyph(ListTodo),
        icon: project.backlog > 0 ? <Text style={styles.menuCount}>{project.backlog}</Text> : undefined,
        onPress: () => open(`/backlog/${conn.host.id}/${encodeURIComponent(project.id)}`),
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
      `Everything between this phone and ${conn?.host.name || 'your desktop'} is end-to-end encrypted. The relay only ever sees ciphertext.${status.kind ? `\n\n${status.text}.` : ''}`,
    );

  return (
    <Screen>
      {/* ket's mark, where Back sits on every other screen, wears the line
          to the desktop: its green when up, grey while reaching, red when
          down. Across from it, the lock on that line, what it says, and
          after a hairline a mark for how it runs: Wi-Fi, VPN or a cloud. */}
      <View style={styles.top}>
        <View style={styles.brand}>
          <BrandMark size={17} color={status.connected ? t.brand : conn?.status === 'offline' ? t.failed : t.faint} />
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
            {status.connected ? 'Secure' : conn?.status === 'offline' ? 'Offline' : 'Reaching'}
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
            {conn?.host.name ?? ''}
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
              value={filter}
              onChange={setFilter}
              segments={[
                { key: 'all' as Filter, label: 'All', count: tally('all') },
                { key: 'running' as Filter, label: '', name: named('Running', 'running'), icon: <Dot color={t.running} size={7} />, count: tally('running') },
                { key: 'waiting' as Filter, label: '', name: named('Waiting', 'waiting'), icon: <Dot color={t.attention} size={7} />, count: tally('waiting') },
                { key: 'idle' as Filter, label: '', name: named('Idle', 'idle'), icon: <Dot color={t.faint} size={7} />, count: tally('idle') },
              ]}
            />
            {conn && status.connected ? (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Start a task"
                onPress={() => open(`/new/${conn.host.id}`)}
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
          >
            {waiting.length > 0 && conn && !filling ? (
              <Pressable style={({ pressed }) => [styles.alertRow, pressed && { backgroundColor: tint(t.attention, 0.14) }]} onPress={answer}>
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
            ) : null}
            {filling ? (
              <View style={styles.card}>
                <ListSkeleton />
              </View>
            ) : null}
            {(filling ? [] : projects).map((project) => {
              const rows = project.worktrees.filter(
                (w) => !gone.includes(w.id) && (filter === 'all' || filterOf(w.activity) === filter),
              );
              // Work this phone just asked for, until its worktree is here.
              const starting =
                filter === 'all' || filter === 'running'
                  ? (conn?.starting ?? []).filter((work) => work.project === project.id)
                  : [];
              // All lists every project, as the desktop's sidebar does —
              // a desktop from 1.9 sends them running or not. The other
              // filters list only projects with a row to show.
              if (rows.length === 0 && starting.length === 0 && filter !== 'all') return null;
              // A desktop from 1.9 keeps its backlogs.
              const keeps = conn !== null && conn.hostMinor >= 9 && project.id !== '';
              const key = projectKey(project);
              const folded = collapsed.includes(key);
              return (
                <View key={key} style={styles.card}>
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
                          ? () => open(`/backlog/${conn.host.id}/${encodeURIComponent(project.id)}`)
                          : undefined
                      }
                    />
                  </View>
                  {starting.map((work) => (
                    <View key={`starting-${work.at}`}>
                      <Row
                        mark={work.agent ? <AgentMark agent={work.agent} size={16} /> : null}
                        title={firstLine(work.prompt)}
                        detail={`${stageText(progressOf(work, conn?.snapshot ?? null), conn?.host.name || 'your desktop', work.agent)} · ${elapsed(work.at, now)}`}
                        state={{ word: 'starting', color: t.brand }}
                        chevron
                        onPress={() => open(`/starting/${conn?.host.id ?? ''}/${work.at}`)}
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
                </View>
              );
            })}
            {empty && conn ? <NothingRunning host={conn.host.name} /> : null}
            {!filling && all.length > 0 && count(filter) === 0 ? <Prose style={styles.note}>No worktree is {filter}.</Prose> : null}
            {!filling && tuckedAway > 0 ? (
              <Pressable
                accessibilityRole="button"
                onPress={() => setTucking(true)}
                style={({ pressed }) => [styles.tucked, pressed && { backgroundColor: t.selection }]}
              >
                <EyeOff size={14} color={t.faint} />
                <Text style={styles.tuckedText}>{tuckedAway} hidden on this phone</Text>
              </Pressable>
            ) : null}
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
        detail={`On this phone only. They stay in ket on ${conn?.host.name || 'your Mac'}, and Start still offers them.`}
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
          {(conns ?? []).map((c) => (
            <SheetItem
              key={c.host.id}
              role="radio"
              label={c.host.name}
              mono
              mark={<Desktop size={18} color={t.text} />}
              detail={statusLine(c).text}
              dot={c.status === 'connected' ? t.running : c.status === 'offline' ? t.failed : t.faint}
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

/** What holding a worktree offers: what the desktop's worktree menu does
 * that the phone can ask for too. Pin first; then its pull request's
 * actions — or, with none open, pushing it and opening one; then Delete.
 * The repository's own checkout is neither pinned nor deleted, and has no
 * pull request of its own. Empty when there is nothing to offer. The
 * worktree's own screen offers the same from its bar. */
export function worktreeMenu(
  conn: HostConnection,
  worktree: WorktreeSummary,
  open: (href: string) => void,
  /** Takes a worktree out of the list while it is deleted, or puts it back. */
  going: (id: string, away: boolean) => void,
): MenuSection[] {
  const pr = worktree.pullRequest;
  const host = conn.host.name || 'your desktop';
  const name = worktree.name || worktree.branch || 'this worktree';
  // A repository's own checkout has no base: it is what the others branch
  // from, and what a pull request would merge into.
  const primary = worktree.base === '';
  // An element, not a call: an icon is a component, and one that uses a hook
  // may only run while React renders it, not while a menu is being built.
  const icon = (glyph: ComponentType<{ size?: number; color?: string }>, color: string = t.dim) =>
    createElement(glyph, { size: 17, color });
  const failed = (title: string) => (why: string | null) => {
    if (why) Alert.alert(title, sentence(why));
  };
  const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const sections: MenuSection[] = [];

  // Pin and Delete came with 1.20.
  const manages = !primary && conn.hostMinor >= 20;
  if (manages) {
    sections.push({
      items: [
        worktree.pinned
          ? {
              key: 'unpin',
              label: 'Unpin',
              icon: icon(PinOff),
              onPress: () => void conn.pinWorktree(worktree.id, false).catch(reason).then(failed("Couldn't unpin it")),
            }
          : {
              key: 'pin',
              label: 'Pin',
              icon: icon(Pin),
              onPress: () => void conn.pinWorktree(worktree.id, true).catch(reason).then(failed("Couldn't pin it")),
            },
      ],
    });
  }

  if (!pr) {
    // Not before 1.16, which opens them.
    if (!primary && conn.hostMinor >= 16) {
      sections.push({
        items: [
          {
            key: 'open-pr',
            label: 'Open pull request…',
            icon: icon(PullRequestGlyph),
            onPress: () => open(`/pull-request/${conn.host.id}/${encodeURIComponent(worktree.id)}`),
          },
        ],
      });
    }
  } else {
    const can = prActions(pr, conn.hostMinor);

    // Handed to the agent: off to its conversation to watch it work, or word
    // of where the handing stands when there is none to open.
    const handOver = async (kind: 'checks' | 'comments') => {
      const handed = await (kind === 'checks' ? conn.fixChecks(worktree.id) : conn.addressComments(worktree.id)).catch(
        (e: unknown) => ({ why: reason(e) }),
      );
      const what = kind === 'checks' ? 'failed checks' : 'review comments';
      if ('why' in handed) {
        Alert.alert(kind === 'checks' ? "Couldn't hand over the checks" : "Couldn't hand over the comments", handed.why);
        return;
      }
      if (handed.starting) {
        Alert.alert('Starting the agent', `ket on ${host} is still starting the agent. It gets the ${what} once it's up.`);
        return;
      }
      const terminal = agentTerminal(worktree);
      if (terminal) open(`/session/${conn.host.id}/${terminal.id.toString()}`);
      else Alert.alert('Handed over', `The agent on ${host} has the ${what}.`);
    };

    const items: MenuItem[] = [];
    if (can.fix) items.push({ key: 'fix', label: 'Fix checks with agent', icon: icon(CheckCircle), onPress: () => void handOver('checks') });
    if (can.address)
      items.push({ key: 'address', label: 'Address comments with agent', icon: icon(Comment), onPress: () => void handOver('comments') });
    if (can.update)
      items.push({
        key: 'update',
        label: 'Update branch',
        icon: icon(Refresh),
        onPress: () => void conn.updatePullRequest(worktree.id).catch(reason).then(failed("Couldn't update the branch")),
      });
    if (can.ready)
      items.push({
        key: 'ready',
        label: 'Ready for review',
        icon: icon(Check),
        onPress: () => void conn.readyPullRequest(worktree.id).catch(reason).then(failed("Couldn't mark it ready")),
      });
    if (can.reopen)
      items.push({
        key: 'reopen',
        label: 'Reopen pull request',
        icon: icon(PullRequestGlyph),
        onPress: () => void conn.reopenPullRequest(worktree.id).catch(reason).then(failed("Couldn't reopen it")),
      });
    if (can.merge)
      items.push({
        key: 'merge',
        label: 'Merge pull request…',
        icon: icon(Merge),
        onPress: () =>
          askMerge(pr, (method) => {
            void (async () => {
              if (!(await confirmIdentity(`Merge #${pr.number.toString()}`))) return;
              failed("Couldn't merge")(await conn.mergePullRequest(worktree.id, method).catch(reason));
            })();
          }),
      });
    if (items.length > 0) sections.push({ title: `Pull request #${pr.number.toString()}`, items });
  }

  if (manages) {
    // Asked first, as the desktop's dialog asks; then it is you. The row goes
    // at once — the desktop answers only once git has walked the checkout,
    // which can take a while — and comes back if the delete is refused. A
    // delete git refuses because work would be lost says so, and offers to
    // go anyway; the row stays out while that is asked.
    const remove = async (force: boolean) => {
      going(worktree.id, true);
      const result = await conn.deleteWorktree(worktree.id, force).catch((e: unknown) => ({ why: reason(e), losesWork: false }));
      if ('keptBranch' in result) {
        if (result.keptBranch) {
          Alert.alert('Worktree deleted', `Kept the branch ${result.keptBranch}: it has unmerged commits.`);
        }
        return;
      }
      if (result.losesWork && !force) {
        const back = () => going(worktree.id, false);
        Alert.alert(
          `Delete ${name} anyway?`,
          sentence(result.why),
          [
            { text: 'Cancel', style: 'cancel', onPress: back },
            { text: 'Delete anyway', style: 'destructive', onPress: () => void remove(true) },
          ],
          { cancelable: true, onDismiss: back },
        );
        return;
      }
      going(worktree.id, false);
      Alert.alert(`Couldn't delete ${name}`, sentence(result.why));
    };
    sections.push({
      items: [
        {
          key: 'delete',
          label: 'Delete…',
          danger: true,
          icon: icon(Trash, t.failed),
          onPress: () =>
            Alert.alert(`Delete ${name}?`, 'Its checkout and its terminals go. Unmerged commits keep their branch.', [
              { text: 'Cancel', style: 'cancel' },
              {
                text: 'Delete',
                style: 'destructive',
                onPress: () => {
                  void (async () => {
                    if (!(await confirmIdentity(`Delete ${name}`))) return;
                    await remove(false);
                  })();
                },
              },
            ]),
        },
      ],
    });
  }
  return sections;
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

/** A reason as a sentence: capitalised, and ended. */
function sentence(text: string): string {
  const said = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(said) ? said : `${said}.`;
}

/** One worktree in the list. At work, it opens up: its name and state over
 * a terminal's well, where the agent's latest step is, and the lines it has
 * changed. Otherwise the sidebar's row. */
function WorktreeRow({
  worktree,
  waking,
  marker,
  onPress,
  onLongPress,
}: {
  worktree: WorktreeSummary;
  /** The desktop is waking its asleep tab for this phone. */
  waking?: boolean;
  marker?: boolean;
  onPress?: () => void;
  onLongPress?: () => void;
}) {
  const agent = agentOf(worktree);
  const mark = agent ? <AgentMark agent={agent} size={16} /> : null;
  if (filterOf(worktree.activity) !== 'running') {
    // Asleep: nothing runs, but the agent's session is there to wake.
    const asleep = worktree.asleep.length > 0 && !worktree.terminals.some((x) => !x.closed);
    return (
      <Row
        marker={marker}
        mark={mark}
        title={worktree.name || 'Other terminals'}
        detail={waking ? 'Waking on your Mac…' : detail(worktree)}
        detailColor={detailColor(worktree.activity)}
        state={waking ? WAKING : (stateOf(worktree.activity) ?? (asleep ? ASLEEP : undefined))}
        live
        meta={
          <>
            {worktree.pinned ? <PinMark /> : null}
            <RowMeta worktree={worktree} />
          </>
        }
        onPress={onPress}
        onLongPress={onLongPress}
      >
        {worktree.subagents.length > 0 ? <Subagents list={worktree.subagents} /> : null}
      </Row>
    );
  }
  const state = stateOf(worktree.activity);
  const step = detail(worktree);
  const pr = worktree.pullRequest;
  const lines = worktree.linesAdded + worktree.linesRemoved > 0;
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      disabled={!onPress && !onLongPress}
      style={({ pressed }) => [styles.panel, marker && { backgroundColor: t.hover }, pressed && { backgroundColor: t.selection }]}
    >
      {marker ? <View style={styles.marker} /> : null}
      <View style={styles.panelHead}>
        <View style={styles.mark}>{mark}</View>
        <Text style={styles.panelName} numberOfLines={1}>
          {worktree.name || 'Other terminals'}
        </Text>
        {worktree.pinned ? <PinMark /> : null}
        {pr ? <Text style={[styles.figures, { color: checksColor(checksState(pr)) }]}>#{pr.number.toString()}</Text> : null}
        {state ? (
          <Fade token={state.word} item={state} style={styles.state}>
            {(s) => (
              <>
                <Dot color={s.color} size={6} />
                <Text style={[styles.stateText, { color: s.color }]}>{s.word}</Text>
              </>
            )}
          </Fade>
        ) : null}
      </View>
      <View style={styles.well}>
        <Wipe token={`${state?.word ?? ''} ${step}`} item={{ color: state?.color ?? t.dim, step }} fill={t.backdrop} style={styles.line}>
          {(it) => (
            <>
              <Text style={[styles.prompt, { color: it.color }]}>›</Text>
              <Text style={styles.step} numberOfLines={1}>
                {it.step}
              </Text>
              <View style={styles.cursor} />
            </>
          )}
        </Wipe>
        {lines ? (
          <Text style={styles.wellFigures}>
            <Text style={{ color: t.added }}>+{worktree.linesAdded}</Text>{' '}
            <Text style={{ color: t.removed }}>−{worktree.linesRemoved}</Text>
          </Text>
        ) : null}
      </View>
      {worktree.subagents.length > 0 ? <Subagents list={worktree.subagents} /> : null}
    </Pressable>
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

/** A pinned worktree's mark: it stays at the top of its project. */
function PinMark() {
  return (
    <View accessible accessibilityLabel="Pinned">
      <Pin size={13} color={t.faint} weight={1.8} />
    </View>
  );
}

/** What ends a row before its state: the pull request its branch is open
 * as, in its checks' colour as ket's sidebar tints its mark, and the lines
 * its uncommitted work adds and removes. Nothing when it has neither. */
function RowMeta({ worktree }: { worktree: WorktreeSummary }) {
  const pr = worktree.pullRequest;
  const lines = worktree.linesAdded + worktree.linesRemoved > 0;
  if (!pr && !lines) return null;
  return (
    <Text style={styles.lines}>
      {pr ? <Text style={{ color: checksColor(checksState(pr)) }}>#{pr.number.toString()}</Text> : null}
      {pr && lines ? '  ' : null}
      {lines ? (
        <>
          <Text style={{ color: t.added }}>+{worktree.linesAdded}</Text>{' '}
          <Text style={{ color: t.removed }}>−{worktree.linesRemoved}</Text>
        </>
      ) : null}
    </Text>
  );
}

/** Most subagents listed under a row; the rest are counted. */
const MOST_SUBAGENTS = 4;


/** The subagents a worktree's agent has running, under its name as ket's
 * sidebar draws them: an elbow each, what it was sent to do, its state. */
function Subagents({ list }: { list: SubagentSummary[] }) {
  const shown = list.slice(0, MOST_SUBAGENTS);
  return (
    <View style={styles.subagents}>
      {shown.map((subagent) => {
        const waiting = subagent.state === SubagentSummary_State.BLOCKED;
        const idle = subagent.state === SubagentSummary_State.IDLE;
        const color = waiting ? t.attention : idle ? t.faint : t.running;
        return (
          <View key={subagent.id} style={styles.subagent}>
            <View style={styles.elbow} />
            <Dot color={color} size={5} />
            <Text style={styles.subagentText} numberOfLines={1}>
              {subagent.label || 'Subagent'}
            </Text>
            {waiting ? <Text style={[styles.subagentState, { color }]}>waiting</Text> : null}
          </View>
        );
      })}
      {list.length > shown.length ? (
        <Text style={styles.subagentMore}>+{list.length - shown.length} more</Text>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  subagents: { gap: 3, paddingTop: 2 },
  subagent: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 18 },
  elbow: {
    width: 9,
    height: 9,
    marginTop: -8,
    borderLeftWidth: 1,
    borderBottomWidth: 1,
    borderBottomLeftRadius: 4,
    borderColor: t.border,
  },
  subagentText: { flex: 1, minWidth: 0, fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  subagentState: { fontFamily: font.chrome, fontSize: size.caption },
  subagentMore: { paddingLeft: 15, fontFamily: font.chrome, fontSize: size.caption, color: t.faint },
  // ket's mark and the lock's pill, over the title.
  top: { height: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: size.gutter },
  // Back's slot on the other screens: 40 square, in 6 from the gutter.
  brand: { width: 40, height: 40, marginLeft: -6, alignItems: 'center', justifyContent: 'center' },
  secure: { height: 32, flexDirection: 'row', alignItems: 'center', gap: 6, paddingLeft: 10, paddingRight: 12, borderRadius: 16 },
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
  // The one way to start work from here, so it wears ket's green — the
  // colour Send fills with when there is something to send.
  new: {
    height: 36,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingLeft: 10,
    paddingRight: 12,
    borderRadius: size.radius,
    backgroundColor: t.brand,
  },
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
  panel: {
    marginVertical: 3,
    gap: 8,
    paddingTop: 10,
    paddingBottom: 9,
    paddingHorizontal: ROW_INSET,
    borderRadius: size.radiusRow,
    backgroundColor: t.sunken,
  },
  marker: { position: 'absolute', left: 0, top: 12, bottom: 12, width: 3, borderRadius: 2, backgroundColor: t.marker },
  panelHead: { flexDirection: 'row', alignItems: 'center', gap: MARK_GAP },
  mark: { width: MARK_SLOT, alignItems: 'center' },
  panelName: { flex: 1, minWidth: 0, fontFamily: font.chrome, fontSize: size.name, color: t.text },
  figures: { fontFamily: font.chrome, fontSize: 12 },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: 11.5 },
  // The terminal's well: darker than the panel, as a terminal sits in ket.
  well: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingVertical: 7,
    paddingHorizontal: 9,
    borderRadius: size.radiusSm,
    backgroundColor: t.backdrop,
  },
  // Only as wide as its words, so the wipe starts at the text's end.
  line: { flexShrink: 1, gap: 6 },
  prompt: { fontFamily: font.code, fontSize: 12 },
  step: { flexShrink: 1, fontFamily: font.code, fontSize: 12, color: t.dim },
  cursor: { width: 6, height: 13, backgroundColor: t.dim },
  wellFigures: { marginLeft: 'auto', paddingLeft: 8, fontFamily: font.chrome, fontSize: 11.5 },
  lines: { fontFamily: font.chrome, fontSize: 12, marginLeft: 8 },
  note: { padding: 8 },
  // Under a project's heading, in from the edge its rows start at.
  idle: { fontSize: size.detail, paddingLeft: 14, paddingBottom: 8 },
  // Under a row standing in for new work: the line that says it is moving.
  startingTrack: { paddingHorizontal: 14, paddingBottom: 8 },
});
