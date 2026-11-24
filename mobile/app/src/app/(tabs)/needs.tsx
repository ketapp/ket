// Needs you: every worktree, on every paired desktop, whose agent is waiting
// on a person — and, under them, the ones that failed. The one list worth
// opening the app for. Each prompt waiting is a card of its own, answered
// right there; its head opens the chat for the conversation around it.

import { Activity } from '@ket/remote';
import { router } from 'expo-router';
import { ScrollView, StyleSheet, View } from 'react-native';

import { HORIZON, Horizon } from '../../components/Horizon';
import { AgentMark } from '../../components/icons';
import { WaitingPrompt } from '../../components/WaitingPrompt';
import { Card, Heading, Prose, Row, Screen, TAB_BAR_ROOM, TabBar } from '../../components/ui';
import { agentOf, colorOf, detail, detailColor, everyWorktree, questionsOf, stateOf } from '../../lib/activity';
import { usePairedHosts } from '../../lib/hosts';
import { sessionColor, size, t } from '../../lib/theme';

export default function Needs() {
  const hosts = usePairedHosts() ?? [];
  const many = hosts.length > 1;
  const of = (activity: Activity) =>
    hosts.flatMap(({ conn, hostState }) =>
      everyWorktree(hostState.snapshot)
        .filter(({ worktree }) => worktree.activity === activity)
        .map((located) => ({ conn, hostState, ...located })),
    );
  const waiting = of(Activity.BLOCKED);
  const failed = of(Activity.FAILED);
  // One card per prompt: several agents in a worktree can each be waiting.
  // A desktop from before 1.13 sends only the newest.
  const prompts = waiting.flatMap((located) =>
    questionsOf(located.worktree).map((question, at) => ({ ...located, question, at })),
  );

  // Empty, the tab is the desktop's welcome picture: a planet rising from
  // under the tab bar, the words in its sky.
  const quiet = waiting.length === 0 && failed.length === 0;
  return (
    <Screen>
      {quiet ? (
        <View style={styles.sky}>
          <Horizon />
          <View style={styles.words}>
            <Prose color={t.text} style={styles.title}>
              Nothing needs you.
            </Prose>
            <Prose style={styles.note}>When an agent asks for permission or stops on a question, it shows up here.</Prose>
          </View>
        </View>
      ) : (
        <ScrollView contentContainerStyle={styles.page}>
          {prompts.length > 0 ? (
            <View style={styles.bare}>
              <Heading color={t.attention} count={prompts.length}>
                Waiting on you
              </Heading>
            </View>
          ) : null}
          {prompts.map(({ conn, project, worktree, question, at }) => (
            <WaitingPrompt
              key={`${conn.id}/${worktree.id}/${question?.terminalId}/${question?.id ?? at}`}
              conn={conn}
              many={many}
              project={project}
              worktree={worktree}
              question={question}
              onOpen={(href) => router.navigate(href)}
            />
          ))}
          {failed.length > 0 ? (
            <Card style={styles.card}>
              <Heading color={t.failed} count={failed.length}>
                Failed
              </Heading>
              {failed.map(({ conn, hostState, project, worktree }) => (
                <Row
                  key={`${conn.id}/${worktree.id}`}
                  mark={agentOf(worktree) ? <AgentMark agent={agentOf(worktree)} size={16} session={colorOf(worktree)} well /> : null}
                  title={worktree.name}
                  titleColor={sessionColor(colorOf(worktree))?.label}
                  detail={[many ? hostState.host.name : null, project.name, detail(worktree)].filter(Boolean).join(' · ')}
                  detailColor={detailColor(worktree.activity)}
                  markdown
                  state={stateOf(worktree.activity)}
                  onPress={() => router.navigate(`/worktree/${conn.id}/${encodeURIComponent(worktree.id)}`)}
                />
              ))}
            </Card>
          ) : null}
        </ScrollView>
      )}
      <TabBar active="needs" needs={waiting.length} />
    </Screen>
  );
}

const styles = StyleSheet.create({
  // No heading above it: the tab bar already says where this is. Starts where
  // the Worktrees tab's page does.
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: TAB_BAR_ROOM },
  card: { padding: 8, gap: 2 },
  // The heading over the prompt cards, at the inset it has inside a card.
  bare: { paddingHorizontal: 6, marginBottom: -4 },
  sky: { flex: 1 },
  // The words sit centred in the sky above the planet's top.
  words: {
    position: 'absolute',
    top: 0,
    left: 0,
    right: 0,
    height: `${HORIZON * 100}%`,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
    gap: 6,
  },
  title: { fontSize: size.title, textAlign: 'center' },
  note: { textAlign: 'center' },
});
