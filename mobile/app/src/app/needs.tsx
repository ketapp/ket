'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// Needs you: every worktree, on every paired desktop, whose agent is waiting
// on a person — and, under them, the ones that failed. The one list worth
// opening the app for.

import { Activity } from '@ket/remote';
import { router } from 'expo-router';
import { ScrollView, StyleSheet, View } from 'react-native';

import { AgentMark, Branch } from '../components/icons';
import { Card, Heading, Prose, Row, Screen, TAB_BAR_ROOM, TabBar } from '../components/ui';
import { detail, detailColor, everyWorktree, stateOf } from '../lib/activity';
import { useHosts } from '../lib/hosts';
import { size, t } from '../lib/theme';

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
        {rows.map(({ conn, project, worktree }) => (
          <Row
            key={`${conn.host.id}/${worktree.id}`}
            mark={worktree.agent ? <AgentMark agent={worktree.agent} size={16} /> : <Branch size={15} color={t.faint} />}
            title={worktree.name}
            detail={[many ? conn.host.name : null, project.name, worktree.question?.subject || detail(worktree)]
              .filter(Boolean)
              .join(' · ')}
            detailColor={detailColor(worktree.activity)}
            state={stateOf(worktree.activity)}
            onPress={() =>
              router.push(
                worktree.question?.terminalId
                  ? `/session/${conn.host.id}/${worktree.question.terminalId.toString()}`
                  : `/worktree/${conn.host.id}/${encodeURIComponent(worktree.id)}`,
              )
            }
          />
        ))}
      </Card>
    );

  return (
    <Screen>
      <ScrollView contentContainerStyle={styles.page}>
        {section('Waiting on you', t.attention, waiting)}
        {section('Failed', t.failed, failed)}
        {waiting.length === 0 && failed.length === 0 ? (
          <Card style={styles.card}>
            <View style={styles.quiet}>
              <Prose color={t.text}>Nothing needs you.</Prose>
              <Prose>When an agent asks for permission or stops on a question, it shows up here.</Prose>
            </View>
          </Card>
        ) : null}
      </ScrollView>
      <TabBar active="needs" needs={waiting.length} />
    </Screen>
  );
}

const styles = StyleSheet.create({
  // No heading above it: the tab bar already says where this is. Starts where
  // the Worktrees tab's page does.
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: TAB_BAR_ROOM },
  card: { padding: 8, gap: 2 },
  quiet: { padding: 8, gap: 4 },
});
