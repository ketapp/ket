// A conversation's turns as the chat draws them: your messages, the agent's
// words (read as markdown, see Markdown.tsx), and the tool lines between, a
// run of them shut under their count. Rows of the chat's list (see
// app/session/[host]/[terminal].tsx), drawn again only when their turn
// changes.

import { type Turn, Turn_Kind } from '@ket/remote';
import { useRecyclingState } from '@shopify/flash-list';
import { Fragment, memo } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { FileChip, ImagesChip, SentFiles } from './Attachments';
import { bubble, StatusMark } from './ChatRows';
import { Linked, Words } from './Markdown';
import { AgentMark, Chevron } from './icons';
import { type Attachment, fileName, splitFiles } from '../lib/attach';
import { font, size, t } from '../lib/theme';

/** A run of tool lines, shut under how many there are and which tools: the
 * agent's words read straight through, and what it ran is a tap away — or
 * the whole of it, as it ran, in the terminal. */
export const Tools = memo(ToolRun, (before, after) =>
  before.onCopy === after.onCopy &&
  before.turns.length === after.turns.length &&
  before.turns.every((turn, index) => turn === after.turns[index]),
);

function ToolRun({ turns, onCopy }: { turns: Turn[]; onCopy: (text: string) => void }) {
  // The list hands a row's cell to another run as it scrolls: shut again
  // for a run that is not this one.
  const [open, setOpen] = useRecyclingState(false, [turns[0]]);
  const count = turns.length === 1 ? '1 tool call' : `${turns.length} tool calls`;
  const names = [...new Set(turns.map((turn) => turn.tool).filter(Boolean))].join(', ');
  return (
    <View style={styles.tools}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={names ? `${count}: ${names}` : count}
        accessibilityState={{ expanded: open }}
        onPress={() => setOpen(!open)}
        style={({ pressed }) => [styles.toolsHead, pressed && styles.held]}
      >
        <View style={open && styles.toolsOpen}>
          <Chevron size={13} color={t.faint} weight={2} />
        </View>
        <Text style={styles.toolText} numberOfLines={1}>
          {count}
          {names ? <Text style={styles.toolNames}>{` · ${names}`}</Text> : null}
        </Text>
      </Pressable>
      {open ? turns.map((turn, index) => <TurnView key={index} turn={turn} agent="" onCopy={onCopy} />) : null}
    </View>
  );
}

/** One turn. Drawn again only when what it shows changes: the chat redraws
 * on every poll and every change on the desktop, and a long conversation
 * drawn whole each time kept the thread from the taps waiting on it. A turn
 * is the same object until it changes — see `join`. */
export const TurnView = memo(OneTurn);

function OneTurn({
  turn,
  agent,
  carried,
  onCopy,
  delivered,
}: {
  turn: Turn;
  agent: string;
  /** The photos and files a message of yours carried, while the phone has
   * them. */
  carried?: Attachment[];
  onCopy: (text: string) => void;
  delivered?: boolean;
}) {
  // A tool line opened to its whole text — shut again when the list hands
  // this cell to another turn.
  const [open, setOpen] = useRecyclingState(false, [turn]);
  switch (turn.kind) {
    case Turn_Kind.USER: {
      // Files the desktop named under the words, as files; images by their
      // count, unless the phone still has their pictures.
      const { words, files } = splitFiles(turn.text);
      const images = carried ? 0 : (turn.images ?? 0);
      const plain = !carried && images === 0 && files.length === 0;
      const said = words ? (
        <Text style={styles.youText}>
          <Linked text={words} />
        </Text>
      ) : null;
      const mark = delivered ? <StatusMark status="delivered" /> : null;
      return (
        <Pressable
          accessibilityHint="Hold to copy"
          delayLongPress={350}
          onLongPress={() => onCopy(words)}
          style={({ pressed }) => [plain ? styles.you : styles.yours, pressed && styles.held]}
        >
          {plain ? (
            <>
              {mark}
              {said}
            </>
          ) : (
            <>
              {carried ? <SentFiles files={carried} /> : files.map((path) => <FileChip key={path} name={fileName(path)} />)}
              {said || images > 0 ? (
                <View style={styles.bubble}>
                  {mark}
                  {images > 0 ? <ImagesChip count={images} /> : null}
                  {said}
                </View>
              ) : (
                mark
              )}
            </>
          )}
        </Pressable>
      );
    }
    case Turn_Kind.TOOL:
      return (
        <Pressable
          accessibilityHint={open ? 'Tap to shorten' : 'Tap to show all of it'}
          onPress={() => setOpen(!open)}
          delayLongPress={350}
          onLongPress={() => onCopy(turn.text)}
          style={({ pressed }) => [styles.tool, pressed && styles.held]}
        >
          <View style={styles.toolMark} />
          <Text style={styles.toolText} numberOfLines={open ? undefined : 2}>
            {turn.text}
          </Text>
        </Pressable>
      );
    default:
      return (
        <Pressable
          accessibilityHint="Hold to copy"
          delayLongPress={350}
          onLongPress={() => onCopy(turn.text)}
          style={styles.agent}
        >
          {agent ? (
            <View style={styles.agentMark}>
              <AgentMark agent={agent} size={14} />
            </View>
          ) : null}
          <View style={styles.agentBody}>
            <Words text={turn.text} onCopy={onCopy} />
          </View>
        </Pressable>
      );
  }
}

const styles = StyleSheet.create({
  you: { ...bubble, alignSelf: 'flex-end', maxWidth: '86%' },
  // A message of yours with photos or files: them, then its bubble.
  yours: { alignSelf: 'flex-end', maxWidth: '86%', alignItems: 'flex-end', gap: 4 },
  bubble,
  youText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 20, color: t.text },
  agent: { flexDirection: 'row', gap: 10, alignItems: 'flex-start' },
  // Centred on the first line of the words beside it — the body's 1pt top
  // padding plus a 21pt line — however many lines they run to.
  agentMark: { height: 22, paddingTop: 1, justifyContent: 'center' },
  agentBody: { flex: 1, minWidth: 0, gap: 8, paddingTop: 1 },
  held: { opacity: 0.7 },
  tool: { flexDirection: 'row', alignItems: 'flex-start', gap: 8, paddingLeft: 24 },
  toolMark: { width: 5, height: 5, borderRadius: 3, marginTop: 6, backgroundColor: t.faint },
  toolText: { flex: 1, minWidth: 0, fontFamily: font.chrome, fontSize: size.detail, color: t.dim },
  tools: { gap: 10 },
  // The chevron centred where a tool line's dot is, its words where theirs start.
  toolsHead: { flexDirection: 'row', alignItems: 'center', gap: 4, paddingLeft: 20 },
  toolsOpen: { transform: [{ rotate: '90deg' }] },
  toolNames: { color: t.faint },
});
