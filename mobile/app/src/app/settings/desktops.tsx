// Settings › Paired desktops: every desktop this phone is paired with, its
// tile saying whether it is connected, each opening its own screen; and
// pairing another from the head.

import { router } from 'expo-router';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { Desktop, Plus } from '../../components/icons';
import { Button, CardNote, Dot, Masthead, Prose, Row, RuledCard, Screen, Tile, tileGlyph } from '../../components/ui';
import { shared } from '../../components/ui/shared';
import { statusLine, useHosts, useHostStates } from '../../lib/hosts';
import { t } from '../../lib/theme';

export default function PairedDesktops() {
  const hostStates = useHostStates(useHosts()) ?? [];
  const connected = hostStates.filter((h) => h.status === 'connected').length;
  const offline = hostStates.filter((h) => h.status === 'offline').length;
  const reaching = hostStates.length - connected - offline;
  const counts = [
    { n: connected, word: 'connected', color: t.brand },
    { n: offline, word: 'offline', color: t.failed },
    { n: reaching, word: 'reaching', color: t.faint },
  ].filter((c) => c.n > 0);
  return (
    <Screen>
      <Masthead
        onBack={() => (router.canGoBack() ? router.back() : router.replace('/settings'))}
        title="Paired desktops"
        line={
          counts.length > 0 ? (
            <View style={styles.counts}>
              {counts.map((c) => (
                <View key={c.word} style={styles.count}>
                  <Dot color={c.color} size={7} />
                  <Text style={shared.headLine}>{`${c.n} ${c.word}`}</Text>
                </View>
              ))}
            </View>
          ) : (
            'None yet'
          )
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
        {hostStates.length > 0 ? (
          <RuledCard>
            {hostStates.map((hostState) => {
              const status = statusLine(hostState);
              const offline = hostState.status === 'offline';
              const tone = status.connected ? t.brand : offline ? t.failed : t.faint;
              return (
                <Row
                  key={hostState.host.id}
                  ruled
                  chevron
                  mark={
                    <Tile
                      tone={tone}
                      label={status.connected ? 'Connected' : offline ? 'Offline' : 'Connecting'}
                    >
                      <Desktop size={tileGlyph()} color={status.connected || offline ? tone : t.dim} weight={2} />
                    </Tile>
                  }
                  title={hostState.host.name}
                  detail={`${status.text} · ${hostState.displayAddress ? hostOf(hostState.displayAddress) : 'trying saved addresses'}`}
                  detailColor={offline ? t.failed : undefined}
                  onPress={() => router.push(`/devices/${hostState.host.id}`)}
                />
              );
            })}
          </RuledCard>
        ) : (
          <Prose>No desktop paired yet.</Prose>
        )}
        <CardNote>
          To pair another, open Settings › Devices › Pair a device on that Mac, then tap Pair and scan its code.
        </CardNote>
      </ScrollView>
    </Screen>
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

const styles = StyleSheet.create({
  page: { paddingHorizontal: 16, paddingTop: 22, paddingBottom: 40 },
  counts: { flexDirection: 'row', alignItems: 'center', gap: 12 },
  count: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  pair: { paddingLeft: 13 },
});
