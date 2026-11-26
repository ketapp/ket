// One worktree's row on Worktrees: the sidebar's row, in its project's ruled
// card. Its agent's tile says who works there and, as a dot on its corner,
// how it is going: the one place the row says its state. While the agent is
// at work, the line under its name is its latest step, after a prompt and
// before a cursor.

import { Activity, type SubagentSummary, SubagentSummary_State, type WorktreeSummary } from '@ket/remote';
import { StyleSheet, Text, View } from 'react-native';

import { Pin, PullRequestGlyph } from './icons';
import { AgentTile, Chip, Dot, Row, TILE, TILE_GAP } from './ui';
import { agentOf, colorOf, detail, detailColor, filterOf, isUnread, stateOf } from '../lib/activity';
import { checksColor, checksState } from '../lib/checks';
import { agentLabel, font, sessionColor, size, t } from '../lib/theme';

/** A state's colour on this screen: an agent at work is ket's one green, the
 * colour of Start. */
export function toneOf(activity: Activity): string | undefined {
  return activity === Activity.WORKING ? t.brand : stateOf(activity)?.color;
}

/** One worktree in the list: the sidebar's row. At work, its line is the
 * agent's latest step as a terminal has it — a `›` in its state's colour, the
 * words, a block cursor — wiped as it changes. Quiet, its line is the faint
 * ink. The row is not marked unread while its agent is still at it. */
export function WorktreeRow({
  worktree,
  waking: woken,
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
  const session = colorOf(worktree);
  const state = stateOf(worktree.activity);
  const working = filterOf(worktree.activity) === 'running';
  // An answer nobody has opened, as the desktop's sidebar says it: the logo
  // and its corner dot in the unread pink, and the line led by "Unread".
  const unread = !working && isUnread(worktree);
  const tone = unread ? t.unread : toneOf(worktree.activity);
  const waking = woken && !working;
  // Asleep: nothing runs, but the agent's session is there to wake.
  const asleep = !working && worktree.asleep.length > 0 && !worktree.terminals.some((x) => !x.closed);
  const quiet = worktree.activity === Activity.QUIET;
  const who = agent ? agentLabel(agent) : 'Terminal';
  return (
    <Row
      ruled
      marker={marker}
      faded={asleep && !waking}
      mark={
        <AgentTile
          agent={agent}
          session={session}
          tone={waking ? t.faint : tone}
          ring={marker ? t.hover : t.card}
          label={unread ? `${who}, unread` : state ? `${who}, ${state.word}` : asleep ? `${who}, asleep` : who}
        />
      }
      title={worktree.name || 'Other terminals'}
      titleColor={sessionColor(session)?.label}
      detail={waking ? 'Waking on your Mac…' : unread ? afterUnread(detail(worktree)) : detail(worktree)}
      lead={unread && !waking ? { word: 'Unread', color: t.unread } : undefined}
      detailColor={working ? undefined : quiet || waking ? t.faint : detailColor(worktree.activity)}
      markdown
      live
      prompt={working ? (tone ?? t.dim) : undefined}
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

/** The agent's words after "Unread", less one that only said it was done,
 * as the desktop drops "ready" after its lead word. */
function afterUnread(words: string): string {
  return /^(ready|done|nothing running)$/i.test(words) ? '' : words;
}

/** A pinned worktree's mark: it stays at the top of its project. */
function PinMark() {
  return (
    <View accessible accessibilityLabel="Pinned">
      <Pin size={13} color={t.faint} weight={1.8} />
    </View>
  );
}

/** What ends a row: the pull request its branch is open as, on a chip, its
 * glyph in its checks' colour as ket's sidebar tints its mark; and the lines
 * its uncommitted work adds and removes. Nothing when it has neither. */
function RowMeta({ worktree }: { worktree: WorktreeSummary }) {
  const pr = worktree.pullRequest;
  const lines = worktree.linesAdded + worktree.linesRemoved > 0;
  if (!pr && !lines) return null;
  const checks = pr ? checksState(pr) : null;
  return (
    <View style={styles.meta}>
      {pr ? (
        <Chip
          label={`Pull request ${pr.number}${checks ? `, checks ${checks}` : ''}`}
          icon={<PullRequestGlyph size={12} color={checksColor(checks)} weight={2.2} />}
        >
          {pr.number.toString()}
        </Chip>
      ) : null}
      {lines ? (
        <Text
          style={styles.lines}
          accessibilityLabel={`${worktree.linesAdded} lines added, ${worktree.linesRemoved} removed`}
        >
          +{short(worktree.linesAdded)} −{short(worktree.linesRemoved)}
        </Text>
      ) : null}
    </View>
  );
}

/** A line count as the row says it: thousands to one place, `1.3k`. */
export function short(n: number): string {
  if (n < 1000) return `${n}`;
  const k = n / 1000;
  return `${k < 10 ? k.toFixed(1).replace(/\.0$/, '') : Math.round(k)}k`;
}

/** Most subagents listed under a row; the rest are counted. */
const MOST_SUBAGENTS = 4;

/** The subagents a worktree's agent has running, under its name as ket's
 * sidebar draws them: an elbow each, what it was sent to do, its state.
 * The tree stands in the mark's column so the label starts on the same
 * edge as the worktree's name and what the agent is doing. */
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
            <View style={styles.tree}>
              <View style={styles.elbow} />
              <Dot color={color} size={5} />
            </View>
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
  subagents: { gap: 3 },
  subagent: { flexDirection: 'row', alignItems: 'center', gap: TILE_GAP, minHeight: 18 },
  tree: {
    width: TILE,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 3,
  },
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
  subagentMore: { paddingLeft: TILE + TILE_GAP, fontFamily: font.chrome, fontSize: size.caption, color: t.faint },
  meta: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  // Quiet, as a row's quiet line is: no colour, the caption's size.
  lines: { fontFamily: font.chrome, fontSize: size.caption, color: t.faint, fontVariant: ['tabular-nums'] },
});
