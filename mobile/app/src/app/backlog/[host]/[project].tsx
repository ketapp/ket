'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// A project's backlog: notes on work not started yet, kept on the desktop —
// the desktop's backlog dialog, on the phone. The field at the top writes a
// new one in a line; a note opens in place to change its words and priority,
// start it as work, mark it done or delete it. Its files stay on the desktop
// and go to the agent with it when it is started.
//
// Each note's priority is a coloured mark at the start of its row, and the
// open notes are listed most pressing first, newest first within a priority.

import { type Backlog, type BacklogNote, BacklogPriority } from '@ket/remote';
import { useFocusEffect, useLocalSearchParams } from 'expo-router';
import { useCallback, useRef, useState } from 'react';
import { Alert, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { KeyboardAwareScrollView } from 'react-native-keyboard-controller';

import { Check, Chevron, ChevronDown, Plus, PriorityMark } from '../../../components/icons';
import { Bar, Button, Card, Heading, Prose, Screen, Tag, field } from '../../../components/ui';
import type { BacklogChange } from '../../../lib/connection';
import { useHost } from '../../../lib/hosts';
import { font, size, t } from '../../../lib/theme';

/** How often the backlog is read again while it is on screen: a note written
 * at the desktop, or one a start has just marked done, shows within this. */
const POLL_MS = 4000;

/** How long a start waits for the desktop to mark its note done before the
 * note's actions come back: the desktop says why on its own screen. */
const START_WAIT_MS = 60_000;

/** The priorities, most pressing first — the order a picker lists them in. */
const PRIORITIES = [BacklogPriority.URGENT, BacklogPriority.HIGH, BacklogPriority.MEDIUM, BacklogPriority.LOW];

const PRIORITY_NAMES: Record<BacklogPriority, string> = {
  [BacklogPriority.UNSPECIFIED]: 'Medium',
  [BacklogPriority.LOW]: 'Low',
  [BacklogPriority.MEDIUM]: 'Medium',
  [BacklogPriority.HIGH]: 'High',
  [BacklogPriority.URGENT]: 'Urgent',
};

/** A note's priority; one from a desktop that keeps none is Medium. */
function priorityOf(note: BacklogNote): BacklogPriority {
  return note.priority === BacklogPriority.UNSPECIFIED ? BacklogPriority.MEDIUM : note.priority;
}

/** `notes` most pressing first, keeping their order within a priority. */
function byPriority(notes: BacklogNote[]): BacklogNote[] {
  return notes
    .map((note, at) => ({ note, at }))
    .sort((a, b) => priorityOf(b.note) - priorityOf(a.note) || a.at - b.at)
    .map(({ note }) => note);
}

export default function BacklogScreen() {
  const { host: hostId, project: raw } = useLocalSearchParams<{ host: string; project: string }>();
  const projectId = decodeURIComponent(raw);
  const conn = useHost(hostId);
  const project = conn?.snapshot?.projects.find((p) => p.id === projectId);
  const [backlog, setBacklog] = useState<Backlog | null>(null);
  const [failing, setFailing] = useState('');
  const [capture, setCapture] = useState('');
  const [capturePriority, setCapturePriority] = useState(BacklogPriority.MEDIUM);
  const [choosing, setChoosing] = useState(false);
  const [adding, setAdding] = useState(false);
  // The note open for changing, by id: one at a time, as on the desktop.
  const [open, setOpen] = useState<string | null>(null);
  // Started from here and not yet marked done by the desktop.
  const [starting, setStarting] = useState<string[]>([]);
  const [showDone, setShowDone] = useState(false);
  // The last backlog read, for a timer to look at after the render it was
  // set in has gone.
  const latest = useRef<Backlog | null>(null);
  const tooOld = conn !== null && conn.status === 'connected' && conn.hostMinor < 9;
  // A desktop from before 1.10 keeps no priorities: none are shown or asked.
  const ranked = conn !== null && conn.hostMinor >= 10;
  const hostName = conn?.host.name ?? 'your desktop';

  // Only while this screen is the one showing, and only while connected: a
  // read that cannot be answered waits ten seconds to say so.
  useFocusEffect(
    useCallback(() => {
      if (!conn || tooOld) return;
      let stopped = false;
      const read = () => {
        if (conn.status !== 'connected') return;
        void conn
          .backlog(projectId)
          .then((reply) => {
            if (stopped) return;
            latest.current = reply;
            setBacklog(reply);
            setFailing('');
          })
          .catch((e: unknown) => {
            if (!stopped) setFailing(reason(e));
          });
      };
      read();
      const timer = setInterval(read, POLL_MS);
      return () => {
        stopped = true;
        clearInterval(timer);
      };
    }, [conn, projectId, tooOld]),
  );

  /** One change to one note; `false` when the desktop would not make it, with
   * why on screen. */
  const edit = async (noteId: string, change: BacklogChange): Promise<boolean> => {
    if (!conn) return false;
    try {
      const reply = await conn.editBacklog(projectId, noteId, change);
      latest.current = reply;
      setBacklog(reply);
      setFailing('');
      return true;
    } catch (e) {
      setFailing(reason(e));
      return false;
    }
  };

  const add = async () => {
    const title = capture.trim();
    if (!title || adding) return;
    setAdding(true);
    const priority = ranked ? capturePriority : undefined;
    if (await edit('', { save: { title, body: '', priority } })) {
      setCapture('');
      setCapturePriority(BacklogPriority.MEDIUM);
      setChoosing(false);
    }
    setAdding(false);
  };

  const start = async (note: BacklogNote) => {
    if (!conn) return;
    setStarting((was) => [...was, note.id]);
    const why = await conn.startNote(projectId, note.id).catch(reason);
    const settle = () => setStarting((was) => was.filter((id) => id !== note.id));
    if (why) {
      settle();
      setFailing(why);
      return;
    }
    setFailing('');
    setTimeout(() => {
      settle();
      const still = latest.current?.notes.some((n) => n.id === note.id && n.doneMs === 0n);
      if (still) setFailing(`ket on ${hostName} has not started it; its window says why`);
    }, START_WAIT_MS);
  };

  const remove = (note: BacklogNote) =>
    Alert.alert(`Delete “${note.title || 'this note'}”?`, note.files.length > 0 ? 'Its files go with it.' : undefined, [
      { text: 'Cancel', style: 'cancel' },
      {
        text: 'Delete',
        style: 'destructive',
        onPress: () => {
          void edit(note.id, { action: 'remove' }).then((done) => {
            if (done) setOpen(null);
          });
        },
      },
    ]);

  const notes = backlog?.notes ?? [];
  const waiting = byPriority(notes.filter((note) => note.doneMs === 0n));
  const done = notes.filter((note) => note.doneMs !== 0n);

  return (
    <Screen>
      <Bar back title="Backlog" detail={project?.name} />
      <KeyboardAwareScrollView contentContainerStyle={styles.page} keyboardShouldPersistTaps="handled" bottomOffset={16}>
        {tooOld ? (
          <Card style={styles.pad}>
            <Prose>ket on {hostName} is older than this app. Update it — rebuild ket and start a new host — to keep its backlog here.</Prose>
          </Card>
        ) : (
          <>
            <View style={styles.capture}>
              <TextInput
                value={capture}
                onChangeText={setCapture}
                placeholder="Write down something to do"
                placeholderTextColor={t.faint}
                returnKeyType="done"
                submitBehavior="submit"
                onSubmitEditing={() => void add()}
                style={styles.captureInput}
              />
              {ranked ? (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`Priority: ${PRIORITY_NAMES[capturePriority]}`}
                  accessibilityState={{ expanded: choosing }}
                  onPress={() => setChoosing(!choosing)}
                  style={({ pressed }) => [styles.capturePriority, (choosing || pressed) && styles.capturePriorityOpen]}
                >
                  <PriorityMark priority={capturePriority} size={16} />
                  <ChevronDown size={13} color={t.dim} weight={2.2} />
                </Pressable>
              ) : null}
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Add to the backlog"
                disabled={!capture.trim() || adding}
                onPress={() => void add()}
                style={[styles.add, capture.trim() ? styles.addReady : null]}
              >
                <Plus size={18} color={capture.trim() ? t.onBrand : t.faint} weight={2.3} />
              </Pressable>
            </View>
            {ranked && choosing ? (
              <PriorityList
                value={capturePriority}
                onPick={(priority) => {
                  setCapturePriority(priority);
                  setChoosing(false);
                }}
              />
            ) : null}
            {failing ? (
              <Prose color={t.failed} style={styles.note}>
                {sentence(failing)}
              </Prose>
            ) : null}
            {backlog === null ? (
              failing ? null : <Prose style={styles.note}>Reading the backlog…</Prose>
            ) : (
              <>
                <Card style={styles.list}>
                  <Heading count={waiting.length}>Open</Heading>
                  {waiting.length === 0 ? (
                    <Prose style={styles.empty}>Nothing waiting. Write down what to do next, and start it when it’s time.</Prose>
                  ) : (
                    waiting.map((note) =>
                      open === note.id ? (
                        <OpenNote
                          key={note.id}
                          note={note}
                          starting={starting.includes(note.id)}
                          hostName={hostName}
                          ranked={ranked}
                          onSave={(title, body, priority) =>
                            edit(note.id, { save: { title, body, priority: ranked ? priority : undefined } })
                          }
                          onStart={() => void start(note)}
                          onDone={() => {
                            void edit(note.id, { action: 'done' }).then((ok) => ok && setOpen(null));
                          }}
                          onDelete={() => remove(note)}
                          onClose={() => setOpen(null)}
                        />
                      ) : (
                        <NoteRow key={note.id} note={note} ranked={ranked} onPress={() => setOpen(note.id)} />
                      ),
                    )
                  )}
                </Card>
                {done.length > 0 ? (
                  <Card style={styles.list}>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityState={{ expanded: showDone }}
                      onPress={() => setShowDone(!showDone)}
                      style={styles.doneHead}
                    >
                      <View style={styles.grow}>
                        <Heading count={done.length}>Done</Heading>
                      </View>
                      <View style={showDone && styles.turned}>
                        <Chevron size={14} color={t.faint} />
                      </View>
                    </Pressable>
                    {showDone
                      ? done.map((note) =>
                          open === note.id ? (
                            <View key={note.id} style={styles.opened}>
                              <NoteText note={note} />
                              <View style={styles.actions}>
                                <Button
                                  title="Reopen"
                                  onPress={() => {
                                    void edit(note.id, { action: 'reopen' }).then((ok) => ok && setOpen(null));
                                  }}
                                  style={styles.grow}
                                />
                                <Button kind="danger" title="Delete" onPress={() => remove(note)} style={styles.grow} />
                              </View>
                            </View>
                          ) : (
                            <NoteRow key={note.id} note={note} ranked={ranked} onPress={() => setOpen(note.id)} />
                          ),
                        )
                      : null}
                  </Card>
                ) : null}
              </>
            )}
          </>
        )}
      </KeyboardAwareScrollView>
    </Screen>
  );
}

/** A note, closed: its priority, its title, the first line of what it says,
 * its files, and for a done one where it went. */
function NoteRow({ note, ranked, onPress }: { note: BacklogNote; ranked: boolean; onPress: () => void }) {
  const finished = note.doneMs !== 0n;
  return (
    <Pressable onPress={onPress} style={({ pressed }) => [styles.row, pressed && { backgroundColor: t.selection }]}>
      {ranked ? <PriorityMark priority={priorityOf(note)} faded={finished} /> : null}
      <View style={styles.grow}>
        <Text style={[styles.title, finished && { color: t.dim }]} numberOfLines={1}>
          {note.title || firstLine(note.body) || 'Untitled'}
        </Text>
        {finished ? (
          <Text style={styles.detail} numberOfLines={1}>
            {note.branch ? `Started on ${note.branch}` : 'Done by hand'} · {day(note.doneMs)}
          </Text>
        ) : note.title && note.body.trim() ? (
          <Text style={styles.detail} numberOfLines={1}>
            {firstLine(note.body)}
          </Text>
        ) : null}
      </View>
      {note.files.length > 0 ? <Tag>{note.files.length === 1 ? '1 file' : `${note.files.length} files`}</Tag> : null}
    </Pressable>
  );
}

/** A done note, opened: what it said, read-only. */
function NoteText({ note }: { note: BacklogNote }) {
  return (
    <View style={styles.text}>
      <Text style={styles.title}>{note.title || 'Untitled'}</Text>
      {note.body.trim() ? <Prose>{note.body.trim()}</Prose> : null}
      <Text style={styles.detail}>
        {note.branch ? `Started on ${note.branch}` : 'Done by hand'} · {day(note.doneMs)}
      </Text>
    </View>
  );
}

/** The priorities to pick from, in a well under the control that opened
 * them: a dropdown that opens in place, so it scrolls with the list. */
function PriorityList({ value, onPick }: { value: BacklogPriority; onPick: (priority: BacklogPriority) => void }) {
  return (
    <View accessibilityRole="menu" style={styles.priorities}>
      {PRIORITIES.map((priority) => {
        const on = priority === value;
        return (
          <Pressable
            key={priority}
            accessibilityRole="menuitem"
            accessibilityState={{ selected: on }}
            onPress={() => onPick(priority)}
            style={({ pressed }) => [styles.priority, (on || pressed) && { backgroundColor: t.selection }]}
          >
            <PriorityMark priority={priority} size={16} />
            <Text style={[styles.priorityName, styles.grow]}>{PRIORITY_NAMES[priority]}</Text>
            {on ? <Check size={16} color={t.text} weight={2.2} /> : null}
          </Pressable>
        );
      })}
    </View>
  );
}

/** An open note, opened: its words and priority to change, then Save — or,
 * unchanged, Start, Done and Delete, the desktop's row of actions. */
function OpenNote({
  note,
  starting,
  hostName,
  ranked,
  onSave,
  onStart,
  onDone,
  onDelete,
  onClose,
}: {
  note: BacklogNote;
  starting: boolean;
  hostName: string;
  ranked: boolean;
  onSave: (title: string, body: string, priority: BacklogPriority) => Promise<boolean>;
  onStart: () => void;
  onDone: () => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  // Held here, not read back from the note: a read of the backlog arriving
  // while this is being typed in must not take the words away.
  const [title, setTitle] = useState(note.title);
  const [body, setBody] = useState(note.body);
  const [priority, setPriority] = useState(priorityOf(note));
  const [choosing, setChoosing] = useState(false);
  const [saving, setSaving] = useState(false);
  const changed =
    title.trim() !== note.title.trim() || body.trim() !== note.body.trim() || (ranked && priority !== priorityOf(note));
  const empty = !title.trim() && !body.trim();

  const save = async () => {
    setSaving(true);
    await onSave(title.trim(), body.trim(), priority);
    setSaving(false);
  };

  return (
    <View style={styles.opened}>
      <View style={styles.openHead}>
        <TextInput
          value={title}
          onChangeText={setTitle}
          placeholder="What the work is"
          placeholderTextColor={t.faint}
          editable={!starting}
          style={[field, styles.grow]}
        />
        <Pressable accessibilityRole="button" accessibilityLabel="Close the note" onPress={onClose} hitSlop={6} style={styles.close}>
          <View style={styles.turned}>
            <Chevron size={14} color={t.faint} />
          </View>
        </Pressable>
      </View>
      {ranked ? (
        <>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`Priority: ${PRIORITY_NAMES[priority]}`}
            accessibilityState={{ expanded: choosing, disabled: starting }}
            disabled={starting}
            onPress={() => setChoosing(!choosing)}
            style={({ pressed }) => [styles.pick, (choosing || pressed) && styles.pickOpen]}
          >
            <PriorityMark priority={priority} size={16} />
            <Text style={[styles.pickText, styles.grow]}>{PRIORITY_NAMES[priority]}</Text>
            <Text style={styles.pickLabel}>Priority</Text>
            <View style={choosing && styles.flipped}>
              <ChevronDown size={15} color={t.dim} weight={2.2} />
            </View>
          </Pressable>
          {choosing ? (
            <PriorityList
              value={priority}
              onPick={(picked) => {
                setPriority(picked);
                setChoosing(false);
              }}
            />
          ) : null}
        </>
      ) : null}
      <TextInput
        value={body}
        onChangeText={setBody}
        placeholder="What the agent should know"
        placeholderTextColor={t.faint}
        multiline
        editable={!starting}
        style={[field, styles.body]}
      />
      {note.files.length > 0 ? (
        <View style={styles.files}>
          {note.files.map((name) => (
            <Tag key={name}>{name}</Tag>
          ))}
          <Prose style={styles.filesNote}>Kept on {hostName}. They go to the agent when the note is started.</Prose>
        </View>
      ) : null}
      {starting ? (
        <Prose style={styles.starting}>Starting on {hostName}…</Prose>
      ) : changed ? (
        <View style={styles.actions}>
          <Button kind="primary" title="Save" disabled={saving || empty} onPress={() => void save()} style={styles.grow} />
          <Button
            kind="ghost"
            title="Cancel"
            disabled={saving}
            onPress={() => {
              setTitle(note.title);
              setBody(note.body);
              setPriority(priorityOf(note));
              setChoosing(false);
            }}
          />
        </View>
      ) : (
        <View style={styles.actions}>
          <Button kind="primary" title="Start" disabled={empty} onPress={onStart} style={styles.grow} />
          <Button title="Done" onPress={onDone} />
          <Button kind="danger" title="Delete" onPress={onDelete} />
        </View>
      )}
    </View>
  );
}

function firstLine(text: string): string {
  return text.trim().split('\n')[0] ?? '';
}

/** A day, as a list says it: "3 Oct". */
function day(ms: bigint): string {
  return new Date(Number(ms)).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
}

function reason(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** A reason from the desktop as a sentence. */
function sentence(text: string): string {
  const s = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?…]$/.test(s) ? s : `${s}.`;
}

const styles = StyleSheet.create({
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: 40 },
  pad: { padding: 14 },
  grow: { flex: 1, minWidth: 0 },
  // The chat's composer, field and button alike: writing a note down is the
  // same gesture as sending a message.
  capture: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  captureInput: {
    flex: 1,
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.sunken,
    paddingHorizontal: 16,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
  },
  add: {
    width: size.control,
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.selection,
    alignItems: 'center',
    justifyContent: 'center',
  },
  addReady: { backgroundColor: t.brand },
  capturePriority: {
    height: size.control,
    borderRadius: size.control / 2,
    backgroundColor: t.sunken,
    borderWidth: 1,
    borderColor: 'transparent',
    paddingHorizontal: 12,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  capturePriorityOpen: { backgroundColor: t.selection, borderColor: t.dim },
  priorities: { padding: 4, gap: 2, borderRadius: size.radiusLg, backgroundColor: t.sunken },
  priority: { height: 44, flexDirection: 'row', alignItems: 'center', gap: 11, paddingHorizontal: 12, borderRadius: size.radius },
  priorityName: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  pick: {
    height: size.control,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 12,
    borderRadius: size.radius,
    backgroundColor: t.sunken,
    borderWidth: 1,
    borderColor: 'transparent',
  },
  pickOpen: { backgroundColor: t.selection, borderColor: t.dim },
  pickText: { fontFamily: font.prose, fontSize: size.body, color: t.text },
  pickLabel: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  flipped: { transform: [{ rotate: '180deg' }] },
  note: { paddingHorizontal: 6 },
  list: { padding: 6, gap: 2 },
  empty: { paddingHorizontal: 8, paddingBottom: 8 },
  row: {
    minHeight: size.row,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 9,
    paddingHorizontal: 10,
    borderRadius: size.radiusRow,
  },
  title: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  detail: { fontFamily: font.prose, fontSize: size.detail, color: t.dim, marginTop: 3 },
  opened: { gap: 8, padding: 8, borderRadius: size.radiusRow, backgroundColor: t.hover },
  openHead: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  close: { width: 32, height: 32, alignItems: 'center', justifyContent: 'center' },
  body: { height: undefined, minHeight: 96, maxHeight: 220, paddingTop: 11, paddingBottom: 11, lineHeight: 20, textAlignVertical: 'top' },
  files: { flexDirection: 'row', flexWrap: 'wrap', gap: 6 },
  filesNote: { width: '100%', fontSize: size.detail, lineHeight: 17 },
  starting: { paddingHorizontal: 4, paddingVertical: 10 },
  actions: { flexDirection: 'row', gap: 8 },
  text: { gap: 6, paddingHorizontal: 4, paddingTop: 2 },
  doneHead: { flexDirection: 'row', alignItems: 'center', paddingRight: 8 },
  turned: { transform: [{ rotate: '90deg' }] },
});
