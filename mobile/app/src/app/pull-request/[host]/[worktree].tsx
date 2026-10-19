'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// A worktree's branch pushed and opened as a pull request, as the desktop's
// Pull request… dialog does it (desktop 1.16): ket commits what is
// uncommitted, pushes the branch, and opens the pull request into its base —
// or, when one is open for the branch already, pushes to that one.
//
// It asks first, always, as the desktop does: the title and description are
// offered, from the backlog note the branch was started from or the branch
// name, and edited here before anything is pushed. A refusal — no remote, a
// conflict, a push the remote turned down — shows under the words and keeps
// them. Done, the screen stays on the number and a link.

import type { PullRequestDraft, PullRequestOpened } from '@ket/remote';
import { router, useLocalSearchParams } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import { Linking, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';

import { ConnectionHelp } from '../../../components/ConnectionHelp';
import { MicButton } from '../../../components/Mic';
import { Bar, Button, Card, Figure, Prose, Screen, Toggle, field } from '../../../components/ui';
import { everyWorktree } from '../../../lib/activity';
import { useDictation } from '../../../lib/dictation';
import { useHost } from '../../../lib/hosts';
import { confirmIdentity } from '../../../lib/lock';
import { font, size, t, tint } from '../../../lib/theme';
import { canDictate } from '../../../lib/voice';

export default function PullRequest() {
  const { host: hostId, worktree: raw } = useLocalSearchParams<{ host: string; worktree: string }>();
  const id = decodeURIComponent(raw);
  const conn = useHost(hostId);
  const found = everyWorktree(conn?.snapshot ?? null).find((l) => l.worktree.id === id);

  const [draft, setDraft] = useState<PullRequestDraft | null>(null);
  const [unread, setUnread] = useState('');
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [asDraft, setAsDraft] = useState(false);
  const [opening, setOpening] = useState(false);
  const [refusal, setRefusal] = useState('');
  const [opened, setOpened] = useState<PullRequestOpened | null>(null);

  // Dictation's view of the description — see lib/dictation.ts.
  const words = useRef('');
  const selection = useRef({ start: 0, end: 0 });
  const write = (next: string) => {
    words.current = next;
    setBody(next);
  };
  const dictation = useDictation({ local: false, draft: words, selection, write, onNote: (note) => setRefusal(note) });

  // The words offered, once: read again, they would undo what was typed.
  const connected = conn?.status === 'connected';
  const asked = useRef(false);
  useEffect(() => {
    if (!conn || !connected || asked.current) return;
    asked.current = true;
    void conn
      .pullRequestDraft(id)
      .then((read) => {
        setDraft(read);
        setTitle(read.title);
        words.current = read.body;
        selection.current = { start: read.body.length, end: read.body.length };
        setBody(read.body);
      })
      .catch((e: unknown) => {
        asked.current = false;
        setUnread(e instanceof Error ? e.message : String(e));
      });
  }, [conn, connected, id]);

  const existing = draft && draft.existingNumber > 0n ? `#${draft.existingNumber.toString()}` : '';

  const open = async () => {
    if (!conn || !draft) return;
    // The mic's last words go in with the rest.
    await dictation.finish();
    const named = title.trim();
    if (!named) {
      setRefusal('A pull request needs a title.');
      return;
    }
    if (!(await confirmIdentity(`Push ${draft.branch} to ${draft.remote}`))) return;
    setOpening(true);
    setRefusal('');
    const result = await conn
      .openPullRequest(id, { title: named, body: words.current, draft: asDraft })
      .catch((e: unknown) => ({ why: e instanceof Error ? e.message : String(e) }));
    setOpening(false);
    if ('why' in result) {
      setRefusal(result.why.charAt(0).toUpperCase() + result.why.slice(1));
      return;
    }
    setOpened(result.opened);
  };

  const name = found?.worktree.name ?? '';

  return (
    <Screen>
      <KeyboardAvoidingView style={styles.fill} behavior="padding">
        <Bar back detail={name} title="Pull request" />
        <ScrollView contentContainerStyle={styles.page} keyboardShouldPersistTaps="handled">
          {opened ? (
            <Done opened={opened} />
          ) : !conn ? null : !connected && !draft ? (
            <ConnectionHelp conn={conn} />
          ) : unread ? (
            <Card style={styles.pad}>
              <Prose color={t.failed}>{unread.charAt(0).toUpperCase() + unread.slice(1)}</Prose>
            </Card>
          ) : !draft ? (
            <Card style={styles.pad}>
              <Prose color={t.dim}>Reading the branch…</Prose>
            </Card>
          ) : !draft.remote ? (
            <Card style={styles.pad}>
              <Prose>
                {found?.project.name ?? 'This project'} has no remote to push {draft.branch} to. Add one on your Mac first.
              </Prose>
            </Card>
          ) : (
            <>
              <Card style={styles.figures}>
                <Figure label="Into" style={styles.grow}>
                  <Text style={styles.branch} numberOfLines={1}>
                    {draft.base}
                  </Text>
                </Figure>
                <Figure label="From" style={styles.wide}>
                  <Text style={styles.branch} numberOfLines={1}>
                    {draft.branch}
                  </Text>
                </Figure>
              </Card>
              <Prose color={t.dim} style={styles.facts}>
                {[
                  draft.changes > 0
                    ? `${draft.changes === 1 ? '1 uncommitted file is' : `${draft.changes} uncommitted files are`} committed first, then the branch is pushed to ${draft.remote}.`
                    : `The branch is pushed to ${draft.remote}.`,
                  existing ? `${existing} is open for it.` : '',
                ]
                  .filter(Boolean)
                  .join(' ')}
              </Prose>

              {/* Kept with one open too, as the desktop keeps them: the one
                  ket remembers may have closed since, and then these words
                  open the next. */}
              <Card style={styles.form}>
                <Text style={styles.label}>Title</Text>
                <TextInput
                  value={title}
                  onChangeText={(next) => {
                    setTitle(next);
                    setRefusal('');
                  }}
                  placeholder="What this changes"
                  placeholderTextColor={t.faint}
                  returnKeyType="next"
                  style={[field, styles.title]}
                />
                <Text style={styles.label}>Description</Text>
                <View style={[styles.well, dictation.hearing && styles.wellHearing]}>
                  <TextInput
                    multiline
                    value={body}
                    onChangeText={(next) => {
                      dictation.letGo();
                      write(next);
                      setRefusal('');
                    }}
                    onSelectionChange={(e) => {
                      selection.current = e.nativeEvent.selection;
                    }}
                    placeholder="Optional"
                    placeholderTextColor={t.faint}
                    style={[styles.body, !canDictate && styles.bodyAlone]}
                  />
                  {canDictate ? (
                    <MicButton
                      listening={dictation.hearing !== null}
                      level={dictation.level}
                      onPress={() => void dictation.toggle()}
                    />
                  ) : null}
                </View>
                <View style={styles.switch}>
                  <Text style={styles.switchText}>Open as a draft</Text>
                  <Toggle on={asDraft} onChange={setAsDraft} label="Open as a draft" />
                </View>
              </Card>

              {refusal ? <Prose color={t.failed} style={styles.facts}>{refusal}</Prose> : null}
              <Button
                kind="primary"
                title={opening ? 'Pushing…' : existing ? `Push to ${existing}` : 'Push and open pull request'}
                disabled={opening || !connected || !title.trim()}
                onPress={() => void open()}
              />
            </>
          )}
        </ScrollView>
      </KeyboardAvoidingView>
    </Screen>
  );
}

/** What it did: the number, where it went, and the way to it. */
function Done({ opened }: { opened: PullRequestOpened }) {
  const number = `#${opened.number.toString()}`;
  const said = [
    opened.committed > 0 ? `Committed ${opened.committed === 1 ? '1 file' : `${opened.committed} files`}, then pushed` : 'Pushed',
    `${opened.branch} to ${opened.remote}.`,
    opened.existed ? `${number} was open already and has the new commits.` : `${number} asks to merge it into ${opened.base}.`,
  ].join(' ');
  return (
    <>
      <Card style={styles.pad}>
        <Text style={styles.done}>{opened.existed ? `Pushed to ${number}` : `Opened ${number}`}</Text>
        <Prose color={t.dim}>{said}</Prose>
      </Card>
      <Button kind="primary" title="Open on GitHub" onPress={() => void Linking.openURL(opened.url)} />
      <Button kind="ghost" title="Done" onPress={() => (router.canGoBack() ? router.back() : router.replace('/'))} />
    </>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  page: { padding: size.gutter, paddingTop: 0, gap: size.gutter, paddingBottom: 32 },
  pad: { padding: 14, gap: 6 },
  grow: { flex: 1 },
  wide: { flex: 1.6 },
  figures: { flexDirection: 'row', gap: 14, padding: 14 },
  branch: { fontFamily: font.chromeMedium, fontSize: 13.5, color: t.text },
  facts: { paddingHorizontal: 4, fontSize: size.detail },
  form: { padding: 12, gap: 8 },
  label: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.dim, paddingHorizontal: 2 },
  title: { marginBottom: 4 },
  well: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    minHeight: 112,
    borderRadius: size.radius,
    backgroundColor: t.sunken,
  },
  // Listening: a faint ring, inside, so nothing moves.
  wellHearing: { boxShadow: `inset 0 0 0 1px ${tint(t.accent, 0.22)}` },
  body: {
    flex: 1,
    minWidth: 0,
    minHeight: 112,
    maxHeight: 260,
    paddingLeft: 12,
    paddingRight: 0,
    paddingTop: 10,
    paddingBottom: 10,
    color: t.text,
    fontFamily: font.prose,
    fontSize: size.body,
    lineHeight: 21,
    textAlignVertical: 'top',
  },
  // With no mic beside it, the words keep clear of the well's edge.
  bodyAlone: { paddingRight: 12 },
  switch: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingTop: 6, paddingHorizontal: 2 },
  switchText: { fontFamily: font.prose, fontSize: size.body, color: t.text },
  done: { fontFamily: font.proseSemibold, fontSize: size.value, color: t.text },
});
