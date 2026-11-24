// Rows in a list: a project's heading over its worktrees, and a row with
// its mark, words and chevron, on the shared mark column.

import { Fragment, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, TouchableOpacity, View, type StyleProp, type TextStyle } from 'react-native';

import { caption, type Run } from '../../lib/markdown';
import { font, size, t, tint } from '../../lib/theme';
import { Chevron, ChevronDown, Pin } from '../icons';
import { Fade, Wipe } from '../Swap';
import { Badge, Dot, TILE } from './marks';
import { shared } from './shared';

/**
 * A project's heading, on the desk over its card: its badge and its name.
 * With `onToggle` it folds the card away; folded, a dot in the attention
 * colour says one of its worktrees is waiting on you, so folding never hides
 * a question.
 *
 * Holding the heading is `onHold`: the project's menu, which has its
 * backlog. A project pinned to the top wears a pin after its name.
 */
export function ProjectHeading({
  name,
  color,
  icon,
  collapsed,
  pinned,
  waiting,
  onToggle,
  onHold,
}: {
  name: string;
  color: string;
  icon?: string;
  collapsed?: boolean;
  pinned?: boolean;
  waiting?: boolean;
  onToggle?: () => void;
  onHold?: () => void;
}) {
  // Pressed, it dims: on the desk, a fill would be a box round it.
  return (
    <TouchableOpacity
      activeOpacity={0.6}
      onPress={onToggle}
      onLongPress={onHold}
      disabled={!onToggle && !onHold}
      accessibilityRole="button"
      accessibilityState={{ expanded: !collapsed }}
      accessibilityHint={onHold ? 'Hold for more' : undefined}
      style={styles.project}
    >
      <Badge name={name} color={color} icon={icon} size={20} />
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
      <View style={styles.projectRest} />
      {collapsed && waiting ? <Dot color={t.attention} size={6} /> : null}
      {onToggle ? (
        collapsed ? (
          <Chevron size={14} color={t.faint} weight={2} />
        ) : (
          <ChevronDown size={14} color={t.faint} weight={2} />
        )
      ) : null}
    </TouchableOpacity>
  );
}

/** A section's label on the desk, over its card: a dot in its signal's
 * colour and its name — "Waiting on you". */
export function SectionLabel({ children, color }: { children: string; color: string }) {
  return (
    <View accessibilityRole="header" style={styles.section}>
      <Dot color={color} size={7} />
      <Text style={styles.sectionText}>{children}</Text>
    </View>
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
  prompt,
  ruled,
  actions,
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
  /** Its agent is at work: the line is a terminal's, a `›` in this colour
   * before the words and a block cursor after them, wiped with them. */
  prompt?: string;
  /** A row in a ruled card (see `RuledCard`): square, so it meets the
   * card's edges and the rules between rows; a 24 pt tile for its mark, 12
   * from its words; its name in medium and its line a step larger. */
  ruled?: boolean;
  /** What a screen reader offers on it beyond a tap: what holding it does. */
  actions?: { name: string; label: string; run: () => void }[];
  onPress?: () => void;
  /** Held: the row's menu. */
  onLongPress?: () => void;
  children?: ReactNode;
}) {
  const detailText = (text: string, color?: string, style?: StyleProp<TextStyle>) =>
    markdown ? (
      <Caption text={text} color={color} style={[ruled && styles.ruledDetail, style]} />
    ) : (
      <Text style={[shared.detail, ruled && styles.ruledDetail, color ? { color } : null, style]} numberOfLines={1}>
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
      {mark !== undefined ? (
        <View style={[ruled ? styles.tileSlot : styles.mark, faded && (ruled ? styles.fadedRuled : styles.faded)]}>{mark}</View>
      ) : null}
      <View style={shared.rowText}>
        <Text
          style={[
            styles.name,
            ruled && styles.ruledName,
            small && { fontFamily: font.prose },
            titleColor ? { color: titleColor } : null,
            faded && { color: t.dim },
          ]}
          numberOfLines={1}
        >
          {title}
        </Text>
        {detail && live ? (
          <Wipe
            token={`${prompt ?? ''} ${detail}`}
            item={{ detail, detailColor, prompt }}
            fill={marker ? t.hover : t.card}
            style={[styles.detailLine, prompt ? styles.promptLine : null]}
          >
            {(it) =>
              it.prompt ? (
                <>
                  <Text style={[styles.prompt, { color: it.prompt }]}>›</Text>
                  {detailText(it.detail, it.detailColor, styles.promptText)}
                  <View style={[styles.cursor, ruled && styles.ruledCursor]} />
                </>
              ) : (
                detailText(it.detail, it.detailColor)
              )
            }
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
      accessibilityActions={actions?.map(({ name, label }) => ({ name, label }))}
      onAccessibilityAction={
        actions ? (event) => actions.find((a) => a.name === event.nativeEvent.actionName)?.run() : undefined
      }
      style={({ pressed }) => [
        styles.row,
        ruled && styles.ruledRow,
        children ? styles.rowWithTree : null,
        small && { minHeight: size.rowSm },
        marker && { backgroundColor: t.hover },
        pressed && { backgroundColor: t.selection },
      ]}
    >
      {marker ? <View style={styles.marker} /> : null}
      {children ? <View style={[styles.rowMain, ruled && styles.ruledRow]}>{main}</View> : main}
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

/** From a ruled row's tile to its words. */
export const TILE_GAP = 12;

// How far a chevron's stroke ends inside its 24-unit box, as a share of its
// size: 8.1 units pointing right. A row's chevron is pulled out by that much,
// so what you see ends at the row's inset, as its badge starts at it.
const CHEVRON_RIGHT = 8.1 / 24;

const styles = StyleSheet.create({
  // On the desk, in 4 from the card's edge under it.
  project: {
    height: 36,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 4,
    borderRadius: size.radiusRow,
  },
  projectLine: { flexShrink: 1, flexDirection: 'row', alignItems: 'center', gap: 6 },
  projectName: { flexShrink: 1, fontFamily: font.proseSemibold, fontSize: size.title, lineHeight: 20, color: t.text },
  projectRest: { flex: 1 },
  section: { height: 28, flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 4 },
  sectionText: { fontFamily: font.proseSemibold, fontSize: size.label, color: t.dim },
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
  ruledRow: { gap: TILE_GAP, borderRadius: 0 },
  tileSlot: { width: TILE, alignItems: 'center' },
  fadedRuled: { opacity: 0.5 },
  ruledName: { fontFamily: font.chromeMedium, lineHeight: 18 },
  ruledDetail: { fontSize: size.label, lineHeight: 18 },
  ruledCursor: { height: 12 },
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
  // A terminal's line: the prompt, the words, the cursor, 6 apart. The words
  // give way first, so the cursor stays on the line however long they run.
  promptLine: { gap: 6 },
  prompt: { fontFamily: font.code, fontSize: 12, lineHeight: 16 },
  promptText: { flexShrink: 1 },
  cursor: { width: 6, height: 13, backgroundColor: t.dim },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: 11.5 },
});
