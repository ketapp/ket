// Settings › Security: the app lock — Face ID, or a PIN, when ket opens and
// before a merge or an "Always" answer — how long ket waits in the
// background before it locks, and changing the PIN.

import { router } from 'expo-router';
import { useEffect, useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { FaceScan, Padlock } from '../../components/icons';
import { CardNote, Masthead, Row, RuledCard, Screen, SectionLabel, Segmented, Tile, Toggle, tileGlyph } from '../../components/ui';
import { shared } from '../../components/ui/shared';
import { LOCK_AFTER, biometry, confirmIdentity, setLockAfter, turnOff, useLock } from '../../lib/lock';
import { t } from '../../lib/theme';

const GLYPH = tileGlyph();

export default function SecuritySettings() {
  const lock = useLock();
  const [kind, setKind] = useState<string | null>(null);
  useEffect(() => {
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
  return (
    <Screen>
      <Masthead
        onBack={() => (router.canGoBack() ? router.back() : router.replace('/settings'))}
        title="Security"
        line={
          <View style={styles.line}>
            <Padlock size={11} color={lock.on ? t.running : t.faint} weight={2.4} />
            <Text style={shared.headLine}>{lock.on ? `App lock on · ${kind ?? 'PIN'}` : 'App lock off'}</Text>
          </View>
        }
      />
      <ScrollView contentContainerStyle={styles.page}>
        <View>
          <RuledCard>
            <Row
              ruled
              mark={
                <Tile>
                  <FaceScan size={GLYPH} color={t.dim} weight={2} />
                </Tile>
              }
              title={kind ? `Require ${kind}` : 'Require a PIN'}
              meta={<Toggle on={lock.on} onChange={(on) => void changeLock(on)} label="App lock" />}
            />
            {lock.on ? (
              <Row
                ruled
                chevron
                mark={
                  <Tile>
                    <Padlock size={GLYPH} color={t.dim} weight={2} />
                  </Tile>
                }
                title="Change PIN"
                onPress={() => void changePin()}
              />
            ) : null}
          </RuledCard>
          <CardNote>
            {kind
              ? `ket asks for ${kind} or your PIN when it opens, and again before a merge or an "Always" answer.`
              : 'ket asks for your PIN when it opens, and again before a merge or an "Always" answer.'}
          </CardNote>
        </View>
        {lock.on ? (
          <View>
            <SectionLabel>Lock after</SectionLabel>
            <Segmented
              segments={LOCK_AFTER.map((choice) => ({ key: choice.key, label: choice.label }))}
              value={String(lock.afterMs)}
              onChange={(key) => void setLockAfter(Number(key))}
              height={36}
              style={styles.cardGap}
            />
            <CardNote>How long ket can be in the background before it locks.</CardNote>
          </View>
        ) : null}
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  page: { paddingHorizontal: 16, paddingTop: 22, gap: 18, paddingBottom: 40 },
  line: { flexDirection: 'row', alignItems: 'center', gap: 5 },
  cardGap: { marginTop: 6 },
});
