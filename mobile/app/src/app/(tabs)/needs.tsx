'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// Needs you: every worktree, on every paired desktop, whose agent is waiting
// on a person — and, under them, the ones that failed. The one list worth
// opening the app for.

import { Activity } from '@ket/remote';
import { router } from 'expo-router';
import { ScrollView, StyleSheet, View } from 'react-native';

import { HORIZON, Horizon } from '../../components/Horizon';
import { AgentMark } from '../../components/icons';
import { Card, Heading, Prose, Row, Screen, TAB_BAR_ROOM, TabBar } from '../../components/ui';
import { agentOf, detail, detailColor, everyWorktree, stateOf } from '../../lib/activity';
import { useHosts } from '../../lib/hosts';
import { size, t } from '../../lib/theme';

export default function Needs() {
  const conns = useHosts() ?? [];
  const many = conns.length > 1;
  const of = (activity: Activity) =>
    conns.flatMap((conn) =>
      everyWorktree(conn.snapshot)
        .filter(({ worktree }) => worktree.activity === activity)
        .map((located) => ({ conn, ...located })),
    );
  const waiting = of(Activity.BLOCKED);
  const failed = of(Activity.FAILED);

  const section = (title: string, color: string, rows: typeof waiting) =>
    rows.length === 0 ? null : (
      <Card style={styles.card}>
        <Heading color={color} count={rows.length}>
          {title}
        </Heading>
        {rows.flatMap(({ conn, project, worktree }) => {
          // One row per prompt: several agents in a worktree can each be
          // waiting. A desktop from before 1.13 sends only the newest.
          const prompts = worktree.questions.length > 0 ? worktree.questions : [worktree.question];
          return prompts.map((question, at) => (
            <Row
              key={`${conn.host.id}/${worktree.id}/${question?.id.toString() ?? at}`}
              mark={agentOf(worktree) ? <AgentMark agent={agentOf(worktree)} size={16} /> : null}
              title={worktree.name}
              detail={[many ? conn.host.name : null, project.name, question?.subject || detail(worktree)]
                .filter(Boolean)
                .join(' · ')}
              detailColor={detailColor(worktree.activity)}
              state={stateOf(worktree.activity)}
              onPress={() =>
                router.push(
                  question?.terminalId
                    ? `/session/${conn.host.id}/${question.terminalId.toString()}`
                    : `/worktree/${conn.host.id}/${encodeURIComponent(worktree.id)}`,
                )
              }
            />
          ));
        })}
      </Card>
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
          {section('Waiting on you', t.attention, waiting)}
          {section('Failed', t.failed, failed)}
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
