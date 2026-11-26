// Settings: ket's own head — its mark and name over the version — and Pair
// across from it, then a row for each part of the settings, saying how it is
// set, each opening a screen of its own as iOS Settings does: the desktops
// this phone is paired with, which notices it shows, the app lock, and
// About. Blackout hours a Mac has set show here too, for as long as it has.

import { Activity } from '@ket/remote';
import { router } from 'expo-router';
import { useEffect, useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { startLabel } from '../../components/AutomationsSheet';
import { Bell, BrandMark, Desktop, FaceScan, Moon, Padlock, Plus } from '../../components/icons';
import {
  Button,
  CardNote,
  Masthead,
  Row,
  RuledCard,
  Screen,
  TAB_BAR_ROOM,
  TabBar,
  Tag,
  Tile,
  tileGlyph,
} from '../../components/ui';
import { shared } from '../../components/ui/shared';
import { everyWorktree } from '../../lib/activity';
import { type Hours, clockOf, nextStart, useBlackout, windowAt } from '../../lib/blackout';
import { useHosts, useHostStates } from '../../lib/hosts';
import { LOCK_AFTER, biometry, useLock } from '../../lib/lock';
import { NOTICES } from '../../lib/notice-kinds';
import { useNotices } from '../../lib/notices';
import { t } from '../../lib/theme';
import { build, version } from '../../lib/version';

const GLYPH = tileGlyph();

export default function Settings() {
  const hostStates = useHostStates(useHosts()) ?? [];
  const lock = useLock();
  const [kind, setKind] = useState<string | null>(null);
  useEffect(() => {
    void biometry().then(setKind);
  }, []);
  const notices = useNotices();
  const blackout = useBlackout();
  const waiting = hostStates.reduce(
    (n, h) => n + everyWorktree(h.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );

  // The desktops' tile wears the best of their lines: green while one is
  // up, red when the rest are down.
  const up = hostStates.some((h) => h.status === 'connected');
  const down = hostStates.some((h) => h.status === 'offline');
  const desktopsTone = up ? t.brand : down ? t.failed : undefined;
  const desktops =
    hostStates.length === 0
      ? 'None paired yet'
      : hostStates
          .map((h) =>
            h.status === 'offline' ? `${h.host.name} offline` : h.status === 'connected' ? h.host.name : `${h.host.name} reaching`,
          )
          .join(' · ');
  const told = NOTICES.filter((notice) => notices.config[notice.kind]).length;
  const after = LOCK_AFTER.find((choice) => choice.ms === lock.afterMs);
  const locking = after ? (after.ms === 0 ? 'locks at once' : `locks after ${after.label}`) : null;

  return (
    <Screen>
      <Masthead
        title="ket"
        titleMark={<BrandMark size={20} />}
        line={
          <View style={styles.versionLine}>
            <Text style={shared.headLine}>{`${version} (${build})`}</Text>
            <Tag color={t.merging}>Beta</Tag>
          </View>
        }
        action={
          <Button
            capsule
            title="Pair"
            icon={<Plus size={15} color={t.text} weight={2.2} />}
            onPress={() => router.push('/pair')}
            style={styles.pair}
          />
        }
      />
      <ScrollView contentContainerStyle={styles.page}>
        <RuledCard>
          <Row
            ruled
            chevron
            mark={
              <Tile tone={desktopsTone}>
                <Desktop size={GLYPH} color={desktopsTone ?? t.dim} weight={2} />
              </Tile>
            }
            title="Paired desktops"
            detail={desktops}
            meta={hostStates.length > 0 ? <Text style={styles.count}>{hostStates.length}</Text> : undefined}
            onPress={() => router.push('/settings/desktops')}
          />
        </RuledCard>
        <RuledCard>
          <Row
            ruled
            chevron
            mark={
              <Tile>
                <Bell size={GLYPH} color={t.dim} weight={2} />
              </Tile>
            }
            title="Notifications"
            detail={
              !notices.config.on
                ? 'Off'
                : notices.refused
                  ? 'Off in this phone’s Settings'
                  : `On · ${told} of ${NOTICES.length} kinds`
            }
            detailColor={notices.config.on && notices.refused ? t.attention : undefined}
            onPress={() => router.push('/settings/notifications')}
          />
          <Row
            ruled
            chevron
            mark={
              <Tile>
                <FaceScan size={GLYPH} color={t.dim} weight={2} />
              </Tile>
            }
            title="Security"
            detail={lock.on ? [kind ?? 'PIN', locking].filter(Boolean).join(' · ') : 'App lock off'}
            onPress={() => router.push('/settings/security')}
          />
        </RuledCard>
        {blackout.hours.length > 0 ? (
          <View>
            <RuledCard>
              {blackout.hours.map((hours) => (
                <Row
                  key={hours.host}
                  ruled
                  mark={
                    <Tile>
                      <Moon size={GLYPH} color={t.dim} weight={2} />
                    </Tile>
                  }
                  title={hoursTitle(hours, blackout.now)}
                  detail={hoursDetail(hours, blackout.now)}
                />
              ))}
            </RuledCard>
            <CardNote>
              {`Set on ${blackout.hours.map((hours) => hours.name).join(' and ')}, under Settings › Devices. ket shuts off on this phone for those hours, and only that Mac can change them or let you in early.`}
            </CardNote>
          </View>
        ) : null}
        <View>
          <RuledCard>
            <Row
              ruled
              chevron
              mark={
                <Tile>
                  <BrandMark size={12} />
                </Tile>
              }
              title="About"
              detail="Version, error reports, website"
              onPress={() => router.push('/settings/about')}
            />
          </RuledCard>
          <CardNote icon={<Padlock size={12} color={t.running} weight={2.4} />}>
            Everything between this phone and your desktop is end-to-end encrypted. The relay only ever sees ciphertext.
          </CardNote>
        </View>
      </ScrollView>
      <TabBar active="settings" needs={waiting} />
    </Screen>
  );
}

/** A desktop's blackout hours as a span: `10:00 PM until 7:00 AM`, from
 * the one on now or the next to start. */
function hoursTitle(hours: Hours, now: number): string {
  const window = windowAt(hours, now) ?? hours.windows.find(([start]) => start > now);
  return window ? `${clockOf(window[0])} until ${clockOf(window[1])}` : 'No blackout coming';
}

/** Which days, and when the next one starts — or, in one the desktop has
 * let this phone in during, until when. */
function hoursDetail(hours: Hours, now: number): string {
  const days = hours.repeat || 'Blackout hours';
  if (windowAt(hours, now)) {
    return now < hours.letInUntil ? `${days} · let in until ${clockOf(hours.letInUntil)}` : `${days} · on now`;
  }
  const next = nextStart(hours, now);
  return next === null ? days : `${days} · next ${startLabel(next, now)}`;
}

const styles = StyleSheet.create({
  // Under the head, at the Worktrees tab's edge; each card 16 under the one
  // above it.
  page: { paddingHorizontal: 16, paddingTop: 22, gap: 16, paddingBottom: TAB_BAR_ROOM },
  versionLine: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  pair: { paddingLeft: 13 },
  count: { ...shared.headLine, fontVariant: ['tabular-nums'] },
});
