// One worktree's row on Worktrees: the sidebar's row, or, while its agent
// is at work, opened up over a terminal's well with the agent's latest step,
// its subagents and the lines it has changed.

import { type SubagentSummary, SubagentSummary_State, type WorktreeSummary } from '@ket/remote';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { Fade, Wipe } from './Swap';
import { AgentMark, Pin } from './icons';
import { Caption, Dot, Row, MARK_GAP, MARK_SLOT, ROW_INSET } from './ui';
import { ASLEEP, WAKING, agentOf, colorOf, detail, detailColor, filterOf, isUnread, stateOf } from '../lib/activity';
import { checksColor, checksState } from '../lib/checks';
import { font, sessionColor, size, t } from '../lib/theme';

/** One worktree in the list. At work, it opens up: its name and state over
 * a terminal's well, where the agent's latest step is, and the lines it has
 * changed. Otherwise the sidebar's row. */
export function WorktreeRow({
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
  const session = colorOf(worktree);
  const mark = agent ? <AgentMark agent={agent} size={16} session={session} well /> : null;
  const titleColor = sessionColor(session)?.label;
  const unread = isUnread(worktree);
  if (filterOf(worktree.activity) !== 'running') {
    // Asleep: nothing runs, but the agent's session is there to wake.
    const asleep = worktree.asleep.length > 0 && !worktree.terminals.some((x) => !x.closed);
    return (
      <Row
        marker={marker}
        mark={mark}
        title={worktree.name || 'Other terminals'}
        titleColor={titleColor}
        detail={waking ? 'Waking on your Mac…' : detail(worktree)}
        detailColor={detailColor(worktree.activity)}
        markdown
        state={waking ? WAKING : (stateOf(worktree.activity) ?? (asleep ? ASLEEP : undefined))}
        live
        meta={
          <>
            {unread ? <UnreadMark /> : null}
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
        <Text style={[styles.panelName, titleColor ? { color: titleColor } : null]} numberOfLines={1}>
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
              <Caption text={it.step} style={styles.step} />
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

/** A pinned worktree's mark: it stays at the top of its project. */
/** The dot on a row whose agent has answered and nobody has opened it since. */
function UnreadMark() {
  return (
    <View accessible accessibilityLabel="Unread">
      <Dot color={t.accent} size={7} />
    </View>
  );
}

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
  subagent: { flexDirection: 'row', alignItems: 'center', gap: MARK_GAP, minHeight: 18 },
  tree: {
    width: MARK_SLOT,
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
  subagentMore: { paddingLeft: MARK_SLOT + MARK_GAP, fontFamily: font.chrome, fontSize: size.caption, color: t.faint },
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
  step: { flexShrink: 1, fontFamily: font.prose, fontSize: 12, color: t.dim },
  cursor: { width: 6, height: 13, backgroundColor: t.dim },
  wellFigures: { marginLeft: 'auto', paddingLeft: 8, fontFamily: font.chrome, fontSize: 11.5 },
  lines: { fontFamily: font.chrome, fontSize: 12, marginLeft: 8 },
});
