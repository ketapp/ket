'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// The chat for work just asked for, before it has a worktree: what the agent
// was asked, as a message on its way, and under it what the desktop is doing
// to get the agent running. Start task lands here the moment the desktop
// takes the request, so asking reads as done — the way sending a message to
// a running agent does — rather than as a row on Worktrees to go and find.
//
// The desktop says nothing of its progress; the snapshot does. A worktree the
// project did not have when it asked, on the branch the prompt names, is the
// worktree being made; that worktree's agent terminal is the chat. When that
// appears this screen becomes the terminal's own chat where it is, with the
// message already in it as sending and whatever was typed here as its draft:
// starting a task moves you once, here, and not again when the agent opens.
// The terminal goes in the address as `?terminal=`, so the chat is still the
// chat when the page is loaded again — the request itself is not kept.
//
// Two minutes without that is too long: the desktop may be busy, the base
// branch slow to fetch, or the worktree made with no agent to open in it. The
// message is shown as held, with the choice to wait on, ask again, or let go.

import { router, useLocalSearchParams, useNavigation } from 'expo-router';
import { useEffect, useState } from 'react';
import { ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Clipboard from 'expo-clipboard';

import { Outgoing, Working } from '../../../components/ChatRows';
import { SendButton } from '../../../components/Composer';
import { KeyboardLift } from '../../../components/KeyboardLift';
import { Track } from '../../../components/Track';
import { Changes } from '../../../components/icons';
import { Bar, Button, Dot, IconCluster, Prose, Screen } from '../../../components/ui';
import { STARTING_FOR_MS } from '../../../lib/connection';
import { useHost } from '../../../lib/hosts';
import { type Seed, elapsed, firstLine, progressOf, segments, stageText } from '../../../lib/starting';
import { Chat } from '../../session/[host]/[terminal]';
import { agentLabel, font, size, t } from '../../../lib/theme';

export default function StartingTask() {
  const { host: hostId, at, terminal: opened } = useLocalSearchParams<{ host: string; at: string; terminal?: string }>();
  const conn = useHost(hostId);
  const insets = useSafeAreaInsets();
  // This screen's own route: another screen may be on top of it when the
  // agent opens, and `router.setParams` would name the terminal in that one.
  const navigation = useNavigation<{ setParams(params: { terminal: string }): void }>();
  // Asked again: this screen follows the new request, keeping its draft.
  const [following, setFollowing] = useState<number | null>(null);
  // From the connection's record of every request, not the row that stands
  // in for one on Worktrees: the row goes the moment the worktree is listed,
  // which can be before this screen has opened.
  const key = following === null ? at : String(following);
  const work = conn?.asked.find((asked) => String(asked.at) === key) ?? null;
  const [now, setNow] = useState(() => Date.now());
  // The terminal whose chat this screen has become — see below.
  const [chat, setChat] = useState<{ terminal: string; seed?: Seed } | null>(() => (opened ? { terminal: opened } : null));
  useEffect(() => {
    if (chat && chat.terminal !== opened) navigation.setParams({ terminal: chat.terminal });
  }, [chat, opened, navigation]);
  useEffect(() => {
    if (chat) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [chat]);
  // Keep waiting: counted again from the press.
  const [since, setSince] = useState<number | null>(null);
  const [draft, setDraft] = useState('');
  const [note, setNote] = useState('');
  const [asking, setAsking] = useState(false);

  const progress = work ? progressOf(work, conn?.snapshot ?? null) : { step: 'asked' as const };
  const waited = now - (since ?? work?.at ?? now);
  const stalled = progress.step !== 'ready' && waited > STARTING_FOR_MS;
  const mac = conn?.host.name || 'your Mac';
  const agent = work?.agent ?? '';

  // Its agent's tab is open: from here on this screen is that terminal's
  // chat, this message already in it, the draft typed here in its field.
  // Set while rendering, as React has state follow what it is drawn from;
  // kept once set, whatever the snapshot or the list of requests says next.
  if (!chat && work && progress.step === 'ready') {
    setChat({ terminal: progress.terminal.toString(), seed: { text: work.prompt, draft } });
  }

  const leave = () => (router.canGoBack() ? router.back() : router.replace('/'));
  const copy = (text: string) => {
    void Clipboard.setStringAsync(text);
    setNote('Copied');
    setTimeout(() => setNote((shown) => (shown === 'Copied' ? '' : shown)), 1500);
  };
  // The same request again — its base, model and effort too — followed from
  // here, so the draft stays.
  const askAgain = async () => {
    if (!conn || !work || asking) return;
    setAsking(true);
    setNote('');
    const why = await conn
      .startWork(work.project, work.prompt, work.agent, work.choices)
      .catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setAsking(false);
    if (why) {
      setNote(why.charAt(0).toUpperCase() + why.slice(1));
      return;
    }
    const again = conn.asked.at(-1);
    if (again) {
      setSince(null);
      setFollowing(again.at);
    }
  };

  // Under the composer at rest: clear of the home indicator rather than the
  // whole safe area, as the session's chat is.
  const rest = Math.max(insets.bottom - 18, 6);
  const lift = rest - 6;

  if (chat) return <Chat started={{ host: hostId, ...chat }} />;
  if (!work) {
    return (
      <Screen>
        <Bar back title="Starting" />
        <View style={styles.gone}>
          <Prose color={t.dim}>This task isn’t starting from this phone any more. Its worktree, if it was made, is on Worktrees.</Prose>
          <Button title="Worktrees" onPress={() => router.replace('/')} />
        </View>
      </Screen>
    );
  }

  const state = stalled ? { word: 'not started', color: t.quotaWarm } : { word: 'starting', color: t.brand };
  const worktree = progress.step === 'asked' ? undefined : progress.worktree;
  const minutes = Math.max(1, Math.round((now - work.at) / 60_000));
  return (
    <Screen>
      <View style={styles.fill}>
        <Bar
          back
          title={worktree?.name || firstLine(work.prompt)}
          lead={
            <View style={styles.state}>
              <Dot size={6} color={state.color} />
              <Text style={[styles.stateText, { color: state.color }]}>{state.word}</Text>
            </View>
          }
          right={
            worktree ? (
              <IconCluster
                actions={[
                  {
                    label: 'The worktree',
                    onPress: () => router.push(`/worktree/${hostId}/${encodeURIComponent(worktree.id)}`),
                    icon: <Changes size={18} color={t.text} />,
                  },
                ]}
              />
            ) : undefined
          }
        />
        <View style={styles.segments} accessible accessibilityLabel={stalled ? 'Not started' : stageText(progress, mac, agent)}>
          {segments(progress, stalled).map((segment, index) => (
            <Track key={index} state={segment} color={stalled ? t.quotaWarm : t.brand} />
          ))}
        </View>
        <ScrollView style={styles.fill} contentContainerStyle={styles.page} keyboardShouldPersistTaps="handled">
          <Outgoing text={work.prompt} status={stalled ? 'held' : 'sending'} onCopy={copy} />
          {stalled ? (
            <View style={styles.stalled}>
              <Prose color={t.dim}>
                {worktree
                  ? `${mac} made the worktree ${minutes} min ago, but no agent has opened in it. Open it to start one there.`
                  : `${mac} took the task ${minutes} min ago but hasn’t made a worktree yet. ket there may be busy, or the base branch slow to fetch.`}
              </Prose>
              <View style={styles.actions}>
                <Button kind="primary" title="Keep waiting" onPress={() => setSince(Date.now())} />
                {worktree ? (
                  <Button title="Open the worktree" onPress={() => router.push(`/worktree/${hostId}/${encodeURIComponent(worktree.id)}`)} />
                ) : (
                  <Button title="Ask again" disabled={asking || conn?.status !== 'connected'} onPress={() => void askAgain()} />
                )}
                <Button kind="danger" title="Forget it" onPress={leave} />
              </View>
            </View>
          ) : (
            <Working agent={agent} what={`${stageText(progress, mac, agent)} · ${elapsed(work.at, now)}`} />
          )}
          {note ? <Prose color={t.dim}>{note}</Prose> : null}
        </ScrollView>

        <KeyboardLift offset={{ closed: 0, opened: lift }}>
          <View style={[styles.composer, { paddingBottom: rest }]}>
            <View style={styles.field}>
              <TextInput
                value={draft}
                onChangeText={setDraft}
                placeholder={`Message ${agent ? agentLabel(agent) : 'the agent'}`}
                placeholderTextColor={t.faint}
                accessibilityLabel="A message for the agent, once it is running"
                multiline
                style={styles.input}
              />
            </View>
            {/* Lit once the agent is running, in the chat this gives way to:
                a message typed before then would land in the shell the
                agent is starting from. */}
            <SendButton ready={false} stop={false} stopping={false} launches={0} onPress={() => {}} />
          </View>
        </KeyboardLift>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  gone: { gap: 12, padding: size.gutter, alignItems: 'flex-start' },
  state: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  stateText: { fontFamily: font.chrome, fontSize: size.caption },
  segments: { flexDirection: 'row', gap: 4, paddingHorizontal: size.gutter, paddingBottom: 10 },
  page: { paddingHorizontal: size.gutter + 4, paddingTop: 4, paddingBottom: 18, gap: 12 },
  stalled: { gap: 12, paddingVertical: 12, alignItems: 'flex-start' },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
  composer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
    paddingHorizontal: size.gutter,
    paddingTop: 8,
  },
  field: { flex: 1, minWidth: 0, minHeight: size.control, borderRadius: size.control / 2, backgroundColor: t.sunken },
  input: {
    flex: 1,
    minWidth: 0,
    minHeight: size.control,
    maxHeight: 140,
    paddingHorizontal: 16,
    paddingTop: 11,
    paddingBottom: 11,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
    lineHeight: 21,
    textAlignVertical: 'top',
  },
});
