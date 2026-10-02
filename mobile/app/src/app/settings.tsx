'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// Settings: the desktops this phone is paired with, and pairing another.

import { router } from 'expo-router';
import { Activity } from '@ket/remote';
import { useEffect, useState } from 'react';
import { Linking, ScrollView, StyleSheet, Text, View } from 'react-native';

import { BrandMark, Desktop, External } from '../components/icons';
import { Button, Card, Heading, Prose, Row, Screen, Segmented, TAB_BAR_ROOM, TabBar, Toggle } from '../components/ui';
import { everyWorktree } from '../lib/activity';
import { statusLine, useHosts } from '../lib/hosts';
import { LOCK_AFTER, biometry, confirmIdentity, setLockAfter, turnOff, useLock } from '../lib/lock';
import { getFit, saveFit } from '../lib/store';
import { font, size, t } from '../lib/theme';
import { build, version } from '../lib/version';

export default function Settings() {
  const conns = useHosts() ?? [];
  const [fit, setFit] = useState(false);
  const lock = useLock();
  const [kind, setKind] = useState<string | null>(null);
  useEffect(() => {
    void getFit().then(setFit);
    void biometry().then(setKind);
  }, []);
  // On: a PIN is chosen first, which is what turns it on. Off, or a new PIN:
  // only once it is you.
  const changeLock = async (on: boolean) => {
    if (on) {
      router.push('/pin');
      return;
    }
    if (await confirmIdentity('Turn off the app lock')) await turnOff();
  };
  const changePin = async () => {
    if (await confirmIdentity('Change your PIN')) router.push('/pin');
  };
  const changeFit = (on: boolean) => {
    setFit(on);
    void saveFit(on);
  };
  const waiting = conns.reduce(
    (n, c) => n + everyWorktree(c.snapshot).filter((l) => l.worktree.activity === Activity.BLOCKED).length,
    0,
  );
  return (
    <Screen>
      <ScrollView contentContainerStyle={styles.page}>
        <View style={styles.brand} accessible accessibilityLabel="ket">
          <BrandMark size={26} />
          <Text style={styles.wordmark}>ket</Text>
        </View>
        <Card style={styles.card}>
          <Heading count={conns.length}>Paired desktops</Heading>
          {conns.map((conn) => (
            <Row
              key={conn.host.id}
              small
              chevron
              mark={<Desktop size={16} />}
              title={conn.host.name}
              detail={`${statusLine(conn).text} · ${hostOf(conn.host.relay)}`}
              onPress={() => router.push(`/devices/${conn.host.id}`)}
            />
          ))}
          {conns.length === 0 ? <Prose style={styles.note}>No desktop paired yet.</Prose> : null}
          <View style={styles.actions}>
            <Button kind="primary" title="Pair a desktop" onPress={() => router.push('/pair')} />
          </View>
        </Card>
        <Card style={styles.card}>
          <Heading>Terminal</Heading>
          <View style={styles.setting}>
            <View style={styles.settingText}>
              <Text style={styles.settingName}>Fit terminal to phone</Text>
              <Prose style={styles.settingDetail}>
                While a terminal is open here, it takes your phone&apos;s size, so text wraps to the screen at a size you can
                read. The desktop gets its own size back when you leave the terminal.
              </Prose>
            </View>
            <Toggle on={fit} onChange={changeFit} label="Fit terminal to phone" />
          </View>
        </Card>
        <Card style={styles.card}>
          <Heading>Security</Heading>
          <View style={styles.setting}>
            <View style={styles.settingText}>
              <Text style={styles.settingName}>{kind ? `Require ${kind}` : 'Require a PIN'}</Text>
              <Prose style={styles.settingDetail}>
                {kind
                  ? `ket asks for ${kind} or your PIN when it opens, and again before a merge or an "Always" answer.`
                  : 'ket asks for your PIN when it opens, and again before a merge or an "Always" answer.'}
              </Prose>
            </View>
            <Toggle on={lock.on} onChange={(on) => void changeLock(on)} label="App lock" />
          </View>
          {lock.on ? (
            <>
              <View style={styles.setting}>
                <View style={styles.settingText}>
                  <Text style={styles.settingName}>Lock after</Text>
                  <Prose style={styles.settingDetail}>How long ket can be in the background before it locks.</Prose>
                </View>
              </View>
              <Segmented
                segments={LOCK_AFTER.map((choice) => ({ key: choice.key, label: choice.label }))}
                value={String(lock.afterMs)}
                onChange={(key) => void setLockAfter(Number(key))}
                style={styles.segmented}
              />
              <Row small chevron title="Change PIN" onPress={() => void changePin()} />
            </>
          ) : null}
        </Card>
        <Card style={styles.card}>
          <Heading>About</Heading>
          <Row small title="Version" meta={<Text style={styles.version}>{`${version} (${build})`}</Text>} />
          <Row
            small
            title="Website"
            meta={
              <View style={styles.link}>
                <Text style={styles.linkText}>ketapp.dev</Text>
                <External size={14} color={t.dim} />
              </View>
            }
            onPress={() => void Linking.openURL(WEBSITE)}
          />
        </Card>
        <Prose style={styles.footnote} color={t.faint}>
          Everything between this phone and your desktop is end-to-end encrypted. The relay only ever sees ciphertext.
        </Prose>
      </ScrollView>
      <TabBar active="settings" needs={waiting} />
    </Screen>
  );
}

/** ket's website, opened in the browser from About. */
const WEBSITE = 'https://ketapp.dev';

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

const styles = StyleSheet.create({
  // No heading above it: the tab bar already says where this is. Starts where
  // the Worktrees tab's page does.
  page: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: TAB_BAR_ROOM },
  card: { padding: 8, gap: 2 },
  note: { padding: 8 },
  actions: { padding: 6, paddingTop: 10, alignItems: 'flex-start' },
  footnote: { fontSize: size.detail, lineHeight: 18, paddingHorizontal: 6 },
  setting: { flexDirection: 'row', alignItems: 'flex-start', gap: 14, padding: 8 },
  settingText: { flex: 1, gap: 4 },
  settingName: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  settingDetail: { fontSize: size.detail, lineHeight: 18 },
  segmented: { marginHorizontal: 8, marginBottom: 6 },
  version: { fontFamily: font.chrome, fontSize: size.label, color: t.dim, paddingRight: 6 },
  link: { flexDirection: 'row', alignItems: 'center', gap: 4, paddingRight: 6 },
  linkText: { fontFamily: font.chrome, fontSize: size.label, color: t.dim },
  brand: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10, paddingTop: 18, paddingBottom: 10 },
  wordmark: { fontFamily: font.chromeSemibold, fontSize: 23, letterSpacing: -0.46, color: t.text },
});
