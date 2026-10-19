'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// One file's changes in a worktree, as a merge would bring them: the lines
// added and removed, each numbered, wrapped to the phone's width rather than
// scrolled sideways.
//
// With a desktop from 1.14 the lines can be commented on, as a pull request
// review's are: tap a line to pick it, and another in the same hunk to pick
// the lines between; Comment writes what should change. Each comment joins
// the worktree's review (lib/review.ts) as a card under its last line, and
// Send to agent hands the whole review to the agent as one message. An older
// desktop's diff is only for looking.

import { useLocalSearchParams } from 'expo-router';
import { useEffect, useMemo, useRef, useState } from 'react';
import { FlatList, Keyboard, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView, useKeyboardState } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { MicButton } from '../../../components/Mic';
import { Bar, Button, Card, Prose, Screen } from '../../../components/ui';
import { everyWorktree } from '../../../lib/activity';
import { useDictation } from '../../../lib/dictation';
import { useHost } from '../../../lib/hosts';
import {
  type Anchor,
  type ReviewComment,
  commentsSaid,
  linesHeading,
  linesLabel,
  pickText,
  review,
  reviewFull,
  sendReview,
  useReview,
} from '../../../lib/review';
import { font, size, t, tint } from '../../../lib/theme';
import { canDictate } from '../../../lib/voice';

/** Most lines drawn; a longer diff is cut here with a note. */
const MOST_LINES = 3000;

/** A line of code: as the diff has it, as it is drawn, its number in the old
 * file and the new — a line added has no old one, a line taken out no new
 * one — and which hunk it is in. */
type Code = {
  kind: 'add' | 'remove' | 'same';
  text: string;
  shown: string;
  old: number | null;
  now: number | null;
  hunk: number;
};
type Line = Code | { kind: 'hunk'; text: string; hunk: number };

/** What the list draws: a line of the diff, or a comment's card under one. */
type Item = { line: Line; at: number } | { card: ReviewComment };

/** The lines picked: where the pick started, and the line it runs to. */
type Pick = { from: number; to: number };

export default function DiffScreen() {
  const { host: hostId, worktree: raw, path } = useLocalSearchParams<{ host: string; worktree: string; path: string }>();
  const id = decodeURIComponent(raw);
  const conn = useHost(hostId);
  const insets = useSafeAreaInsets();
  const keyboardUp = useKeyboardState((state) => state.isVisible);
  const [lines, setLines] = useState<Line[] | null>(null);
  const [cut, setCut] = useState(false);
  const [error, setError] = useState('');
  const comments = useReview(hostId, id);
  const [pick, setPick] = useState<Pick | null>(null);
  // The composer, while it is open: for the lines picked, or for a comment
  // already in the review.
  const [composing, setComposing] = useState<{ editing: ReviewComment | null } | null>(null);
  const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
  const [sending, setSending] = useState(false);
  const list = useRef<FlatList<Item>>(null);
  // The line kept just above the composer while the keyboard takes room.
  const follow = useRef<number | null>(null);

  useEffect(() => {
    if (!conn || !path) return;
    let stopped = false;
    conn
      .diff(id, path)
      .then((reply) => {
        if (stopped) return;
        if (reply.error) {
          setError(reply.error);
          return;
        }
        const parsed = parse(reply.text);
        setLines(parsed.slice(0, MOST_LINES));
        setCut(reply.truncated || parsed.length > MOST_LINES);
      })
      .catch((e: unknown) => {
        if (!stopped) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stopped = true;
    };
  }, [conn, id, path]);

  // A desktop from before 1.14 cannot be sent a review.
  const canReview = conn !== null && conn.hostMinor >= 14;
  const editing = composing?.editing ?? null;

  // Each comment on this file under the line it ends on, while the diff
  // still has that line; the one being edited gives way to the composer.
  // Where each line's row is, for keeping one in view.
  const { items, rowOf } = useMemo(() => {
    const under = new Map<number, ReviewComment[]>();
    if (lines) {
      for (const comment of comments) {
        if (comment.path !== path || comment.id === editing?.id) continue;
        const at = anchored(lines, comment.anchor);
        if (at >= 0) under.set(at, [...(under.get(at) ?? []), comment]);
      }
    }
    const items: Item[] = [];
    const rowOf: number[] = [];
    (lines ?? []).forEach((line, at) => {
      rowOf.push(items.length);
      items.push({ line, at });
      for (const card of under.get(at) ?? []) items.push({ card });
    });
    return { items, rowOf };
  }, [lines, comments, path, editing]);

  const low = pick ? Math.min(pick.from, pick.to) : -1;
  const high = pick ? Math.max(pick.from, pick.to) : -1;
  const picked: Code[] = pick && lines ? lines.slice(low, high + 1).filter(isCode) : [];
  const label = picked.length > 0 ? labelOf(picked) : '';

  const keepInView = () => {
    const line = follow.current;
    if (line === null || rowOf[line] === undefined) return;
    list.current?.scrollToIndex({ index: rowOf[line], viewPosition: 1, animated: false });
  };

  // A tap picks a line; another in the same hunk runs the pick to it; one in
  // another hunk starts again there; one on a line already picked lets go.
  const tap = (at: number) => {
    const line = lines?.[at];
    if (!lines || !line || line.kind === 'hunk' || composing) return;
    setSaid(null);
    if (!pick) setPick({ from: at, to: at });
    else if (at >= low && at <= high) setPick(null);
    else if (lines[pick.from]?.hunk === line.hunk) setPick({ from: pick.from, to: at });
    else setPick({ from: at, to: at });
  };

  const openComposer = (editing: ReviewComment | null, line: number) => {
    // Full already: say so before anything is written, and let go of the
    // lines so Send to agent is there under it.
    const full = editing ? null : reviewFull(hostId, id, null);
    setSaid(full ? { text: full, failed: true } : null);
    if (full) {
      setPick(null);
      return;
    }
    follow.current = line;
    setComposing({ editing });
  };

  const closeComposer = () => {
    follow.current = null;
    setComposing(null);
    Keyboard.dismiss();
  };

  // Why it could not be added, for the composer to say; `null` once it is.
  const add = (comment: string): string | null => {
    if (editing) {
      const full = reviewFull(hostId, id, { ...editing, comment }, editing.id);
      if (full) return full;
      review.edit(hostId, id, editing.id, comment);
    } else if (path && lines && picked.length > 0) {
      const last = picked[picked.length - 1];
      const anchor: Anchor =
        last.now !== null
          ? { side: 'new', number: last.now, text: last.text }
          : { side: 'old', number: last.old ?? 0, text: last.text };
      const next = { path, lines: label, text: pickText(picked), comment };
      const full = reviewFull(hostId, id, next);
      if (full) return full;
      review.add(hostId, id, { ...next, anchor });
      // The lines are taken; what marks them now is the comment's card.
      setPick(null);
    }
    closeComposer();
    return null;
  };

  const send = async () => {
    if (!conn) return;
    setSending(true);
    setSaid(null);
    setSaid(await sendReview(conn, hostId, id));
    setSending(false);
  };

  const name = path?.split('/').pop() ?? 'Diff';
  const folder = path?.includes('/') ? path.slice(0, path.lastIndexOf('/')) : undefined;
  const local = /^(opencode:ollama\/|ollama:)/i.test(
    everyWorktree(conn?.snapshot ?? null).find((l) => l.worktree.id === id)?.worktree.agent ?? '',
  );

  // At the foot, while there is something to do there: the composer, what
  // to do with the lines picked, or the review to send.
  const footer = !canReview ? null : composing && lines ? (
    <Composer
      key={editing?.id ?? 'new'}
      heading={linesHeading(editing?.lines ?? label)}
      initial={editing?.comment ?? ''}
      editing={editing !== null}
      local={local}
      onAdd={add}
      onCancel={() => {
        // A comment given up on lets go of its lines too.
        if (!editing) setPick(null);
        closeComposer();
      }}
    />
  ) : pick && label ? (
    <View style={styles.actions}>
      <Button
        kind="primary"
        title={`Comment on ${label}`}
        style={styles.grow}
        onPress={() => openComposer(null, high)}
      />
      <Button kind="ghost" title="Clear" onPress={() => setPick(null)} />
    </View>
  ) : comments.length > 0 ? (
    <View style={styles.actions}>
      <Prose style={styles.grow}>{`${commentsSaid(comments.length)} in the review`}</Prose>
      <Button
        kind="primary"
        title={sending ? 'Sending…' : 'Send to agent'}
        disabled={sending}
        onPress={() => void send()}
      />
    </View>
  ) : null;

  return (
    <Screen full>
      <KeyboardAvoidingView style={styles.fill} behavior="padding">
        <Bar back title={name} detail={folder} />
        {error ? (
          <Prose style={styles.message}>{error.charAt(0).toUpperCase() + error.slice(1)}</Prose>
        ) : lines === null ? (
          <Prose style={styles.message} color={t.dim}>
            Loading…
          </Prose>
        ) : lines.length === 0 ? (
          <Prose style={styles.message} color={t.dim}>
            No line changes — a binary file, or only its mode changed.
          </Prose>
        ) : (
          <FlatList
            ref={list}
            style={styles.fill}
            data={items}
            extraData={`${low}:${high}:${composing ? 1 : 0}`}
            keyExtractor={(item) => ('card' in item ? `c${item.card.id}` : String(item.at))}
            renderItem={({ item }) =>
              'card' in item ? (
                <CommentCard
                  comment={item.card}
                  actions={!composing}
                  onEdit={() => {
                    const at = lines ? anchored(lines, item.card.anchor) : -1;
                    if (at >= 0) openComposer(item.card, at);
                  }}
                  onDelete={() => review.remove(hostId, id, [item.card.id])}
                />
              ) : (
                <LineView
                  line={item.line}
                  picked={item.at >= low && item.at <= high}
                  onPress={canReview ? () => tap(item.at) : undefined}
                />
              )
            }
            initialNumToRender={60}
            windowSize={11}
            contentContainerStyle={styles.page}
            keyboardShouldPersistTaps="handled"
            // The keyboard coming up shrinks the list, a frame at a time;
            // the lines commented on stay just above the composer.
            onLayout={keepInView}
            onScrollToIndexFailed={(info) => {
              list.current?.scrollToOffset({ offset: info.averageItemLength * info.index, animated: false });
              setTimeout(keepInView, 50);
            }}
            ListFooterComponent={
              cut ? (
                <Prose style={styles.message} color={t.dim}>
                  The rest is too long to show here; see it on your Mac.
                </Prose>
              ) : null
            }
          />
        )}
        {footer || (canReview && said) ? (
          <Card style={[styles.panel, { marginBottom: keyboardUp ? 6 : Math.max(insets.bottom, size.gutter) }]}>
            {said ? (
              <Prose color={said.failed ? t.failed : undefined} style={styles.said}>
                {said.text}
              </Prose>
            ) : null}
            {footer}
          </Card>
        ) : null}
      </KeyboardAvoidingView>
    </Screen>
  );
}

function LineView({ line, picked, onPress }: { line: Line; picked: boolean; onPress?: () => void }) {
  if (line.kind === 'hunk') {
    return (
      <View style={styles.hunk}>
        <Text style={styles.hunkText} numberOfLines={1}>
          {line.text}
        </Text>
      </View>
    );
  }
  const color = line.kind === 'add' ? t.added : line.kind === 'remove' ? t.removed : undefined;
  return (
    <Pressable
      onPress={onPress}
      disabled={!onPress}
      accessibilityState={onPress ? { selected: picked } : undefined}
      style={[styles.line, color ? { backgroundColor: tint(color, 0.1) } : null, picked && styles.picked]}
    >
      <Text style={[styles.number, color ? { color } : null]}>{line.now ?? line.old}</Text>
      <Text style={[styles.sign, color ? { color } : null]}>
        {line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}
      </Text>
      <Text style={styles.text}>{line.shown || ' '}</Text>
    </Pressable>
  );
}

/** A comment in the review, under its lines: which they are, what it says,
 * and Edit and Delete — left out while the composer is open. */
function CommentCard({
  comment,
  actions,
  onEdit,
  onDelete,
}: {
  comment: ReviewComment;
  actions: boolean;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <View style={styles.card}>
      <View style={styles.cardHead}>
        <Text style={[styles.cardHeading, styles.grow]}>{linesHeading(comment.lines)}</Text>
        {actions ? (
          <>
            <Button kind="ghost" title="Edit" onPress={onEdit} style={styles.cardButton} />
            <Button kind="ghost" title="Delete" onPress={onDelete} style={styles.cardButton} />
          </>
        ) : null}
      </View>
      <Text style={styles.cardText}>{comment.comment}</Text>
    </View>
  );
}

/** Where a comment is written, at the foot over the keyboard: which lines,
 * the words, the mic, and Cancel and Add. Its own state, so typing redraws
 * this and not the diff. */
function Composer({
  heading,
  initial,
  editing,
  local,
  onAdd,
  onCancel,
}: {
  heading: string;
  initial: string;
  editing: boolean;
  /** A local agent's: dictation keeps its words on the phone, or refuses. */
  local: boolean;
  /** Why it could not be added, or `null` once it is. */
  onAdd: (comment: string) => string | null;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const [why, setWhy] = useState('');
  // Dictation's view of the words — see lib/dictation.ts.
  const draft = useRef(initial);
  const selection = useRef({ start: initial.length, end: initial.length });
  const write = (next: string) => {
    draft.current = next;
    setText(next);
  };
  const dictation = useDictation({ local, draft, selection, write, onNote: (note) => setWhy(note) });

  const add = async () => {
    // The mic's last words go in with the rest.
    await dictation.finish();
    const comment = draft.current.trim();
    if (!comment) {
      setWhy('Write what should change first.');
      return;
    }
    setWhy(onAdd(comment) ?? '');
  };

  return (
    <View style={styles.composer}>
      <Text style={styles.cardHeading}>{heading}</Text>
      <View style={[styles.field, dictation.hearing && styles.fieldHearing]}>
        <TextInput
          autoFocus
          multiline
          value={text}
          onChangeText={(next) => {
            dictation.letGo();
            write(next);
            setWhy('');
          }}
          onSelectionChange={(e) => {
            selection.current = e.nativeEvent.selection;
          }}
          placeholder="What should change?"
          placeholderTextColor={t.faint}
          style={[styles.input, !canDictate && styles.inputAlone]}
        />
        {canDictate ? (
          <MicButton listening={dictation.hearing !== null} level={dictation.level} onPress={() => void dictation.toggle()} />
        ) : null}
      </View>
      {why ? <Prose color={t.failed}>{why}</Prose> : null}
      <View style={styles.composerActions}>
        <Button
          kind="ghost"
          title="Cancel"
          style={styles.grow}
          onPress={() => {
            dictation.letGo();
            onCancel();
          }}
        />
        <Button kind="primary" title={editing ? 'Save' : 'Add to review'} style={styles.grow} onPress={() => void add()} />
      </View>
    </View>
  );
}

function isCode(line: Line): line is Code {
  return line.kind !== 'hunk';
}

/** Which lines, as the desktop says them: by their numbers in the new file,
 * or in the old one when every line picked was taken out. */
function labelOf(picked: Code[]): string {
  const now = picked.flatMap((line) => (line.now !== null ? [line.now] : []));
  const old = picked.flatMap((line) => (line.old !== null ? [line.old] : []));
  const numbers = now.length > 0 ? now : old;
  return numbers.length > 0 ? linesLabel(numbers[0], numbers[numbers.length - 1]) : '';
}

/** The line a comment's card goes under, or -1 when the diff has moved on
 * from it. */
function anchored(lines: Line[], anchor: Anchor): number {
  return lines.findIndex(
    (line) =>
      isCode(line) &&
      line.text === anchor.text &&
      (anchor.side === 'new' ? line.now === anchor.number : line.now === null && line.old === anchor.number),
  );
}

/** A unified diff as lines to draw: the file's headers dropped, each hunk
 * marked where it starts, each line numbered in the old file and the new. */
function parse(diff: string): Line[] {
  const lines: Line[] = [];
  let old = 0;
  let now = 0;
  let hunks = 0;
  let inHunk = false;
  for (const raw of diff.split('\n')) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@ ?(.*)$/.exec(raw);
    if (hunk) {
      old = Number(hunk[1]);
      now = Number(hunk[2]);
      hunks += 1;
      inHunk = true;
      lines.push({ kind: 'hunk', text: hunk[3] ? `Line ${now} · ${hunk[3]}` : `Line ${now}`, hunk: hunks });
      continue;
    }
    if (!inHunk || raw.startsWith('\\')) continue;
    if (raw.startsWith('diff --git')) {
      inHunk = false;
      continue;
    }
    const text = raw.slice(1);
    const shown = text.replace(/\t/g, '  ');
    if (raw.startsWith('+')) lines.push({ kind: 'add', text, shown, old: null, now: now++, hunk: hunks });
    else if (raw.startsWith('-')) lines.push({ kind: 'remove', text, shown, old: old++, now: null, hunk: hunks });
    else if (raw.startsWith(' ')) lines.push({ kind: 'same', text, shown, old: old++, now: now++, hunk: hunks });
  }
  return lines;
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  grow: { flex: 1 },
  page: { paddingBottom: 24 },
  message: { paddingHorizontal: size.gutter + 4, paddingVertical: 16 },
  hunk: {
    marginTop: 10,
    paddingHorizontal: size.gutter + 4,
    paddingVertical: 6,
    backgroundColor: t.sunken,
  },
  hunkText: { fontFamily: font.chrome, fontSize: size.caption, color: t.dim },
  line: { flexDirection: 'row', paddingHorizontal: 6 },
  picked: { backgroundColor: t.selection },
  number: {
    width: 36,
    textAlign: 'right',
    fontFamily: font.code,
    fontSize: 11,
    lineHeight: 18,
    color: t.faint,
  },
  sign: { width: 16, textAlign: 'center', fontFamily: font.code, fontSize: 12, lineHeight: 18, color: t.faint },
  text: { flex: 1, minWidth: 0, fontFamily: font.code, fontSize: 12, lineHeight: 18, color: t.text },
  // A comment under its lines: a note beside the code, in from its numbers.
  card: {
    marginVertical: 6,
    marginLeft: 58,
    marginRight: size.gutter,
    padding: 10,
    gap: 6,
    borderRadius: size.radiusCard,
    borderWidth: 1,
    borderColor: t.rule,
    backgroundColor: t.card,
  },
  cardHead: { flexDirection: 'row', alignItems: 'center', gap: 4, minHeight: 32 },
  cardHeading: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.dim },
  cardButton: { height: 32, paddingHorizontal: 10 },
  cardText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 20, color: t.text },
  panel: { marginHorizontal: size.gutter, marginTop: 6, padding: 8, gap: 6 },
  said: { paddingHorizontal: 6, paddingTop: 2 },
  actions: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  composer: { gap: 8, padding: 4 },
  composerActions: { flexDirection: 'row', gap: 8 },
  field: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    minHeight: size.control,
    borderRadius: 20,
    backgroundColor: t.sunken,
  },
  // Listening: a faint ring, inside, so nothing moves.
  fieldHearing: { boxShadow: `inset 0 0 0 1px ${tint(t.accent, 0.22)}` },
  input: {
    flex: 1,
    minWidth: 0,
    minHeight: size.control,
    maxHeight: 140,
    paddingLeft: 14,
    paddingRight: 0,
    paddingTop: 11,
    paddingBottom: 11,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
    lineHeight: 21,
    textAlignVertical: 'top',
  },
  // With no mic beside it, the words keep clear of the field's round end.
  inputAlone: { paddingRight: 14 },
});
