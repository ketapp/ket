// Settings › Notifications: whether this phone shows ket's notices at all,
// what that means, and which of them — each with its own switch.

import { router } from 'expo-router';
import type { ReactNode } from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';

import { Bell, Check, Exclaim, Gauge, Inbox, PullRequestGlyph } from '../../components/icons';
import { CardNote, Masthead, Row, RuledCard, Screen, SectionLabel, Tile, Toggle, tileGlyph } from '../../components/ui';
import { NOTICES } from '../../lib/notice-kinds';
import { type Kind, choose, openPhoneSettings, turnOff, turnOn, useNotices } from '../../lib/notices';
import { t } from '../../lib/theme';

const GLYPH = tileGlyph();

/** Each notice's mark, as the screens it is about draw it. */
const MARKS: Record<Kind, ReactNode> = {
  waiting: <Inbox size={GLYPH} color={t.dim} weight={2} />,
  finished: <Check size={GLYPH} color={t.dim} weight={2.2} />,
  checks: <PullRequestGlyph size={GLYPH} color={t.dim} weight={2} />,
  limits: <Gauge size={GLYPH} color={t.dim} weight={2} />,
};

export default function NotificationSettings() {
  const notices = useNotices();
  const on = notices.config.on;
  const told = NOTICES.filter((notice) => notices.config[notice.kind]).length;
  const change = async (next: boolean) => {
    if (next) await turnOn();
    else await turnOff();
  };
  return (
    <Screen>
      <Masthead
        onBack={() => (router.canGoBack() ? router.back() : router.replace('/settings'))}
        title="Notifications"
        line={on ? `On · ${told} of ${NOTICES.length} kinds` : 'Off'}
      />
      <ScrollView contentContainerStyle={styles.page}>
        <View>
          <RuledCard>
            <Row
              ruled
              mark={
                <Tile>
                  <Bell size={GLYPH} color={t.dim} weight={2} />
                </Tile>
              }
              title="Notify me"
              detail="Banners and a badge on this phone"
              meta={<Toggle on={on} onChange={(next) => void change(next)} label="Notify me" />}
            />
            {on && notices.refused ? (
              <Row
                ruled
                chevron
                mark={
                  <Tile tone={t.attention}>
                    <Exclaim size={GLYPH} color={t.attention} weight={2.4} />
                  </Tile>
                }
                title="Open this phone's Settings"
                detail="Notifications are turned off for ket there"
                detailColor={t.attention}
                onPress={openPhoneSettings}
              />
            ) : null}
          </RuledCard>
          <CardNote>
            A banner while ket is not in front, and a badge counting the prompts waiting on you. Once ket is closed, your Mac
            sends them through ket’s push service, sealed so only this phone can read them.
          </CardNote>
        </View>
        {on ? (
          <View>
            <SectionLabel>Tell me when</SectionLabel>
            <RuledCard style={styles.cardGap}>
              {NOTICES.map((notice) => (
                <Row
                  key={notice.kind}
                  ruled
                  wrap
                  mark={<Tile>{MARKS[notice.kind]}</Tile>}
                  title={notice.name}
                  detail={notice.detail}
                  meta={
                    <Toggle
                      on={notices.config[notice.kind]}
                      onChange={(next) => void choose(notice.kind, next)}
                      label={notice.name}
                    />
                  }
                />
              ))}
            </RuledCard>
          </View>
        ) : null}
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  page: { paddingHorizontal: 16, paddingTop: 22, gap: 18, paddingBottom: 40 },
  cardGap: { marginTop: 6 },
});
