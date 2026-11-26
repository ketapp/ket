// Needs you: every worktree, on every paired desktop, whose agent is waiting
// on a person — and, under them, the ones that failed. The one list worth
// opening the app for. Under the same head as Worktrees and Usage. Each
// prompt waiting is a card of its own, answered right there; its head opens
// the chat for the conversation around it.

import { Activity } from '@ket/remote';
import { router, useFocusEffect } from 'expo-router';
import { useCallback, useState } from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';

import { HostHeader } from '../../components/HostHeader';
import { Inbox } from '../../components/icons';
import { WaitingPrompt } from '../../components/WaitingPrompt';
import {
  AgentTile,
  Empty,
  Row,
  RuledCard,
  Screen,
  SectionLabel,
  TAB_BAR_HEIGHT,
  TAB_BAR_ROOM,
  TabBar,
} from '../../components/ui';
import { agentOf, colorOf, detail, everyWorktree, questionsOf } from '../../lib/activity';
import type { HostConnection } from '../../lib/connection';
import { usePairedHosts } from '../../lib/hosts';
import { useSplit } from '../../lib/layout';
import { getDesktop, heldDesktop, saveDesktop } from '../../lib/store';
import { agentLabel, sessionColor, t } from '../../lib/theme';

export default function Needs() {
  const paired = usePairedHosts();
  const hosts = paired ?? [];
  const many = hosts.length > 1;
  // The head names the desktop the Worktrees tab shows; the list under it is
  // every desktop's.
  const [chosen, setChosen] = useState<string | null>(heldDesktop);
  useFocusEffect(
    useCallback(() => {
      void getDesktop().then(setChosen);
    }, []),
  );
  const choose = (conn: HostConnection) => {
    setChosen(conn.id);
    void saveDesktop(conn.id);
  };
  const current = hosts.find((h) => h.conn.id === chosen) ?? hosts[0] ?? null;
  // Beside the sidebar on a wide screen, which already wears this head.
  const split = useSplit();
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

  const quiet = waiting.length === 0 && failed.length === 0;
  return (
    <Screen>
      {split ? null : (
        <HostHeader hosts={paired} current={current} onChoose={choose} onStart={(c) => router.navigate(`/new/${c.id}`)} />
      )}
      {quiet ? (
        <View style={styles.quiet}>
          <Empty icon={<Inbox size={20} color={t.faint} />} title="Nothing needs you">
            When an agent asks for permission or stops on a question, it shows up here.
          </Empty>
        </View>
      ) : (
        <ScrollView contentContainerStyle={styles.page}>
          {prompts.length > 0 ? (
            <View>
              <SectionLabel color={t.attention}>Waiting on you</SectionLabel>
              <View style={styles.cards}>
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
              </View>
            </View>
          ) : null}
          {failed.length > 0 ? (
            <View>
              <SectionLabel color={t.failed}>Failed</SectionLabel>
              <RuledCard style={styles.cardGap}>
                {failed.map(({ conn, hostState, worktree }) => {
                  const agent = agentOf(worktree);
                  return (
                    <Row
                      key={`${conn.id}/${worktree.id}`}
                      ruled
                      chevron
                      mark={
                        <AgentTile
                          agent={agent}
                          session={colorOf(worktree)}
                          tone={t.failed}
                          label={`${agent ? agentLabel(agent) : 'Agent'}, failed`}
                        />
                      }
                      title={worktree.name}
                      titleColor={sessionColor(colorOf(worktree))?.label}
                      detail={[many ? hostState.host.name : null, detail(worktree)].filter(Boolean).join(' · ')}
                      detailColor={t.failed}
                      markdown
                      onPress={() => router.navigate(`/worktree/${conn.id}/${encodeURIComponent(worktree.id)}`)}
                    />
                  );
                })}
              </RuledCard>
            </View>
          ) : null}
        </ScrollView>
      )}
      <TabBar active="needs" needs={waiting.length} />
    </Screen>
  );
}

const styles = StyleSheet.create({
  // Under the head, at the Worktrees tab's edge; each section 18 under the
  // one above it.
  page: { paddingHorizontal: 16, paddingTop: 18, gap: 18, paddingBottom: TAB_BAR_ROOM },
  // The cards under their label, 10 apart.
  cards: { marginTop: 6, gap: 10 },
  cardGap: { marginTop: 6 },
  // Nothing waiting: the note centred between the head and the tab bar.
  quiet: { flex: 1, paddingBottom: TAB_BAR_HEIGHT },
});
