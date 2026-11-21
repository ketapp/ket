// Rename a worktree, as the desktop's Rename… does (desktop 1.33): the name
// ket shows for it on the Mac and here, not its branch or its folder. Left
// empty, it goes back to the branch's name.
//
// A sheet over whatever opened it, which stays drawn behind it — the
// worktree's menu, on Worktrees or on its own screen. It animates itself in
// and out, as New task does (see new/[host].tsx), and leaves the stack once
// it is down.

import { router, useLocalSearchParams } from 'expo-router';
import { useState } from 'react';
import { StyleSheet, TextInput } from 'react-native';

import { SheetPanel } from '../../../components/Sheet';
import { Button, Prose, field, oneLine } from '../../../components/ui';
import { everyWorktree } from '../../../lib/activity';
import { useHost, useHostState } from '../../../lib/hosts';
import { size, t } from '../../../lib/theme';
import { sentence } from '../../../lib/text';

export default function RenameWorktree() {
  const { host: hostId, worktree: raw } = useLocalSearchParams<{ host: string; worktree: string }>();
  const id = decodeURIComponent(raw);
  const conn = useHost(hostId);
  const hostState = useHostState(conn);
  const worktree = everyWorktree(hostState?.snapshot ?? null).find((l) => l.worktree.id === id)?.worktree;
  // What is typed; until then, what it is called now, as the desktop's
  // field starts. The connection and its snapshot arrive after the first
  // draw, so the field waits for the worktree rather than starting empty.
  const [typed, setTyped] = useState<string | null>(null);
  const name = typed ?? worktree?.name ?? '';
  const [saving, setSaving] = useState(false);
  const [refusal, setRefusal] = useState('');
  const [leaving, setLeaving] = useState(false);

  const save = async () => {
    if (!conn || !worktree || saving) return;
    setSaving(true);
    const why = await conn.renameWorktree(id, name).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setSaving(false);
    if (why) setRefusal(sentence(why));
    else setLeaving(true);
  };
  const gone = () => {
    if (router.canGoBack()) router.back();
    else router.replace('/');
  };

  return (
    <SheetPanel
      open={!leaving}
      onClose={() => setLeaving(true)}
      onGone={gone}
      title="Rename worktree"
      detail={worktree?.branch ? `Git branch: ${worktree.branch}` : undefined}
    >
      {worktree ? (
        <TextInput
          value={name}
          onChangeText={(next) => {
            setTyped(next);
            setRefusal('');
          }}
          placeholder={worktree.branch || 'Worktree name'}
          placeholderTextColor={t.faint}
          {...oneLine}
          autoFocus
          selectTextOnFocus
          autoCorrect={false}
          returnKeyType="done"
          onSubmitEditing={() => void save()}
          accessibilityLabel="Worktree name"
          style={field}
        />
      ) : (
        <Prose color={t.faint} style={styles.refusal}>
          {hostState?.snapshot ? `This worktree is no longer on ${hostState.host.name || 'your desktop'}.` : 'Connecting…'}
        </Prose>
      )}
      {refusal ? (
        <Prose color={t.failed} style={styles.refusal}>
          {refusal}
        </Prose>
      ) : null}
      <Button
        kind="primary"
        title={saving ? 'Renaming…' : 'Rename'}
        disabled={saving || !conn || !worktree}
        onPress={() => void save()}
      />
    </SheetPanel>
  );
}

const styles = StyleSheet.create({
  refusal: { fontSize: size.detail },
});
