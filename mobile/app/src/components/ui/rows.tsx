// Rows in a list: a project's heading over its worktrees, and a row with
// its mark, words and chevron, on the shared mark column.

import { Fragment, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';

import { caption, type Run } from '../../lib/markdown';
import { font, size, t, tint } from '../../lib/theme';
import { Chevron, ListTodo, Pin } from '../icons';
import { Fade, Wipe } from '../Swap';
import { Badge, Dot } from './marks';
import { shared } from './shared';

/**
 * A project's heading, at the top of its card: its badge and its name. With
 * `onToggle` it folds the rows away; folded, the worktrees it holds are named
 * under it, and a dot in the attention colour says one of them is waiting on
 * you, so folding never hides a question. No count of its rows: they are
 * right there, or named.
 *
 * With `onBacklog`, open backlog notes show as a count a tap opens, as the
 * desktop's sidebar has them. Holding the heading is `onHold`: the project's
 * menu. A project pinned to the top wears a pin after its name.
 */
export function ProjectHeading({
  name,
  color,
  icon,
  collapsed,
  inside,
  pinned,
  waiting,
  onToggle,
  onHold,
  backlog = 0,
  onBacklog,
}: {
  name: string;
  color: string;
  icon?: string;
  collapsed?: boolean;
  /** Its worktrees' names, said under it while it is folded. */
  inside?: string;
  pinned?: boolean;
  waiting?: boolean;
  onToggle?: () => void;
  onHold?: () => void;
  backlog?: number;
  onBacklog?: () => void;
}) {
  return (
    <Pressable
      onPress={onToggle}
      onLongPress={onHold}
      disabled={!onToggle && !onHold}
      accessibilityRole="button"
      accessibilityState={{ expanded: !collapsed }}
      accessibilityHint={onHold ? 'Hold for more' : undefined}
      style={({ pressed }) => [styles.project, pressed && { backgroundColor: t.selection }]}
    >
      <Badge name={name} color={color} icon={icon} />
      <View style={styles.projectText}>
        <View style={styles.projectLine}>
          <Text style={styles.projectName} numberOfLines={1}>
            {name}
          </Text>
          {pinned ? (
            <View accessible accessibilityLabel="Pinned">
              <Pin size={13} color={t.faint} weight={1.8} />
            </View>
          ) : null}
        </View>
        {collapsed && inside ? (
          <Text style={styles.projectInside} numberOfLines={1}>
            {inside}
          </Text>
        ) : null}
      </View>
      {collapsed && waiting ? <Dot color={t.attention} size={6} /> : null}
      {onBacklog && backlog > 0 ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`Backlog, ${backlog} open`}
          onPress={onBacklog}
          hitSlop={6}
          style={({ pressed }) => [shared.readout, styles.backlog, pressed && { backgroundColor: t.selection }]}
        >
          <ListTodo size={13} color={t.dim} weight={2} />
          <Text style={shared.count}>{backlog > 99 ? '99+' : backlog}</Text>
        </Pressable>
      ) : null}
      {onToggle ? (
        <View style={collapsed ? styles.fold : styles.open}>
          <Chevron size={14} color={t.faint} />
        </View>
      ) : null}
    </Pressable>
  );
}

/** A state a row ends in: the word, in its colour, after its dot. */
export interface RowState {
  word: string;
  color: string;
}

/**
 * One row: a rounded fill with no hairline. A mark, a name in mono, a line of
 * prose under it, and the row's state at the end. Children sit under that
 * line at the row's full width, so a subagent tree can stand in the mark's
 * column and its label start where the name does. `marker` is the amber rail
 * of the row in force — the desktop the picker is showing — and the only place
 * the marker colour is spent.
 */
export function Row({
  mark,
  title,
  titleColor,
  detail,
  detailColor,
  markdown,
  state,
  meta,
  marker,
  chevron,
  small,
  faded,
  live,
  onPress,
  onLongPress,
  children,
}: {
  mark?: ReactNode;
  title: string;
  /** The title's own colour: a session's `/color`, as `sessionColor`
   * gives it. */
  titleColor?: string;
  detail?: string;
  detailColor?: string;
  /** The detail is an agent's one-line report: paired backticks, `**strong**`
   * and `*emphasis*` draw, rather than as marks. */
  markdown?: boolean;
  state?: RowState;
  meta?: ReactNode;
  marker?: boolean;
  chevron?: boolean;
  small?: boolean;
  /** Something kept but not running — an asleep tab: its mark faded and its
   * name dim. */
  faded?: boolean;
  /** Its detail and state change under a wipe as updates land, as a
   * worktree's do, rather than at once. */
  live?: boolean;
  onPress?: () => void;
  /** Held: the row's menu. */
  onLongPress?: () => void;
  children?: ReactNode;
}) {
  const detailText = (text: string, color?: string) =>
    markdown ? (
      <Caption text={text} color={color} />
    ) : (
      <Text style={[shared.detail, color ? { color } : null]} numberOfLines={1}>
        {text}
      </Text>
    );
  const stateMark = (s: RowState) => (
    <>
      <Dot color={s.color} size={6} />
      <Text style={[styles.stateText, { color: s.color }]}>{s.word}</Text>
    </>
  );
  const main = (
    <>
      {mark !== undefined ? <View style={[styles.mark, faded && styles.faded]}>{mark}</View> : null}
      <View style={shared.rowText}>
        <Text
          style={[styles.name, small && { fontFamily: font.prose }, titleColor ? { color: titleColor } : null, faded && { color: t.dim }]}
          numberOfLines={1}
        >
          {title}
        </Text>
        {detail && live ? (
          <Wipe token={detail} item={{ detail, detailColor }} fill={marker ? t.hover : t.card} style={styles.detailLine}>
            {(it) => detailText(it.detail, it.detailColor)}
          </Wipe>
        ) : detail ? (
          detailText(detail, detailColor)
        ) : null}
      </View>
      {meta}
      {state && live ? (
        <Fade token={state.word} item={state} style={styles.state}>
          {stateMark}
        </Fade>
      ) : state ? (
        <View style={styles.state}>{stateMark(state)}</View>
      ) : null}
      {chevron ? (
        <View style={styles.rowChevron}>
          <Chevron size={16} color={t.faint} />
        </View>
      ) : null}
    </>
  );
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      disabled={!onPress && !onLongPress}
      style={({ pressed }) => [
        styles.row,
        children ? styles.rowWithTree : null,
        small && { minHeight: size.rowSm },
        marker && { backgroundColor: t.hover },
        pressed && { backgroundColor: t.selection },
      ]}
    >
      {marker ? <View style={styles.marker} /> : null}
      {children ? <View style={styles.rowMain}>{main}</View> : main}
      {children}
    </Pressable>
  );
}

/** A one-line status: the agent's own words, with the small amount of
 * Markdown they carry drawn rather than shown as marks. */
export function Caption({
  text,
  color,
  style,
  numberOfLines = 1,
}: {
  text: string;
  color?: string;
  style?: StyleProp<TextStyle>;
  numberOfLines?: number;
}) {
  return (
    <Text style={[shared.detail, color ? { color } : null, style]} numberOfLines={numberOfLines}>
      <CaptionRuns runs={caption(text)} ink={color ?? t.dim} />
    </Text>
  );
}

function CaptionRuns({ runs, ink }: { runs: Run[]; ink: string }) {
  return (
    <>
      {runs.map((piece, at) => {
        switch (piece.kind) {
          case 'text':
            return <Fragment key={at}>{piece.text}</Fragment>;
          case 'code':
            return (
              <Text key={at} style={[styles.captionCode, { backgroundColor: tint(ink, 0.12) }]}>
                {piece.text}
              </Text>
            );
          case 'strong':
          case 'em':
            return (
              <Text key={at} style={piece.kind === 'strong' ? styles.captionStrong : styles.captionEm}>
                <CaptionRuns runs={piece.runs} ink={ink} />
              </Text>
            );
          default:
            return null;
        }
      })}
    </>
  );
}

/** A row's inset from its edge to its mark's slot. */
export const ROW_INSET = 14;

/** The slot a row's mark is centred in: a project badge's width, so an
 * agent's mark under a project heading sits on the badge's centreline. */
export const MARK_SLOT = 22;

/** From the slot to the row's words. */
export const MARK_GAP = 8;

// How far a chevron's stroke ends inside its 24-unit box, as a share of its
// size: 8.1 units pointing right, 5.1 turned down. A row's chevron is pulled
// out by that much, so what you see ends at the row's inset, as its badge
// starts at it.
const CHEVRON_RIGHT = 8.1 / 24;

const CHEVRON_DOWN = 5.1 / 24;

const styles = StyleSheet.create({
  // Its badge stands in the same slot a row's mark does, so the badge and
  // the marks under it share one column and every name starts at the
  // same inset. It ends at that inset too.
  project: {
    minHeight: 50,
    flexDirection: 'row',
    alignItems: 'center',
    gap: MARK_GAP,
    paddingHorizontal: ROW_INSET,
    borderRadius: size.radiusRow,
  },
  fold: { marginRight: -14 * CHEVRON_RIGHT },
  open: { marginRight: -14 * CHEVRON_DOWN, transform: [{ rotate: '90deg' }] },
  backlog: { gap: 5, paddingHorizontal: 8 },
  projectText: { flex: 1, minWidth: 0, gap: 2 },
  projectLine: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  projectName: { flexShrink: 1, fontFamily: font.proseSemibold, fontSize: size.title, color: t.text },
  projectInside: { fontFamily: font.chrome, fontSize: 11.5, color: t.faint },
  row: {
    minHeight: size.row,
    flexDirection: 'row',
    alignItems: 'center',
    gap: MARK_GAP,
    paddingVertical: 9,
    paddingHorizontal: ROW_INSET,
    borderRadius: size.radiusRow,
  },
  // Subagents hang under the name as their own row, so their tree can stand
  // in the mark's column and their label starts where the name does.
  rowWithTree: { flexDirection: 'column', alignItems: 'stretch', gap: 3 },
  rowMain: { flexDirection: 'row', alignItems: 'center', gap: MARK_GAP },
  rowChevron: { marginRight: -16 * CHEVRON_RIGHT },
  marker: { position: 'absolute', left: 0, top: 12, bottom: 12, width: 3, borderRadius: 2, backgroundColor: t.marker },
  mark: { width: MARK_SLOT, alignItems: 'center' },
  faded: { opacity: 0.55 },
  name: { fontFamily: font.chrome, fontSize: size.name, color: t.text },
  captionCode: { fontFamily: font.code },
  captionStrong: { fontFamily: font.proseMedium },
  captionEm: { fontStyle: 'italic' },
  // Only as wide as its words, so the wipe starts at the text's end.
  detailLine: { alignSelf: 'flex-start', maxWidth: '100%' },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: 11.5 },
});
