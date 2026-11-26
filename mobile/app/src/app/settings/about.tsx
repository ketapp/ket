// Settings › About: which ket this is, the error reports kept on this phone
// to share or clear, ket's website, and the line on what the relay can see.

import { router, useFocusEffect } from 'expo-router';
import { Linking, ScrollView, Share, StyleSheet, Text, View } from 'react-native';

import { BrandMark, External, Globe, Info, Padlock, Trash } from '../../components/icons';
import { CardNote, Masthead, Row, RuledCard, Screen, Tag, Tile, tileGlyph } from '../../components/ui';
import { shared } from '../../components/ui/shared';
import { reportText, useErrorReports } from '../../lib/crashes';
import { t } from '../../lib/theme';
import { build, version } from '../../lib/version';

const GLYPH = tileGlyph();

/** ket's website, opened in the browser. */
const WEBSITE = 'https://ketapp.dev';

export default function About() {
  // Errors kept on this phone, read again each time About comes into view.
  const errors = useErrorReports();
  useFocusEffect(errors.refresh);
  const reports = errors.reports.length;
  return (
    <Screen>
      <Masthead
        onBack={() => (router.canGoBack() ? router.back() : router.replace('/settings'))}
        title="About"
        line="ket on this device"
      />
      <ScrollView contentContainerStyle={styles.page}>
        <RuledCard>
          <Row
            ruled
            mark={
              <Tile>
                <BrandMark size={12} />
              </Tile>
            }
            title="Version"
            meta={
              <View style={styles.version}>
                <Tag color={t.merging}>Beta</Tag>
                <Text style={styles.figure}>{`${version} (${build})`}</Text>
              </View>
            }
          />
          <Row
            ruled
            mark={
              <Tile>
                <Info size={GLYPH} color={t.dim} weight={2} />
              </Tile>
            }
            title="Error reports"
            detail="Kept on this phone until you share them"
            meta={<Text style={styles.figure}>{reports === 0 ? 'None' : String(reports)}</Text>}
            chevron={reports > 0}
            onPress={reports > 0 ? () => void Share.share({ message: reportText(errors.reports) }) : undefined}
          />
          {reports > 0 ? (
            <Row
              ruled
              mark={
                <Tile>
                  <Trash size={GLYPH} color={t.dim} weight={2} />
                </Tile>
              }
              title="Clear error reports"
              onPress={errors.clear}
            />
          ) : null}
          <Row
            ruled
            mark={
              <Tile>
                <Globe size={GLYPH} color={t.dim} weight={2} />
              </Tile>
            }
            title="Website"
            meta={
              <View style={styles.link}>
                <Text style={styles.figure}>ketapp.dev</Text>
                <External size={13} color={t.dim} weight={2} />
              </View>
            }
            onPress={() => void Linking.openURL(WEBSITE)}
          />
        </RuledCard>
        <CardNote icon={<Padlock size={12} color={t.running} weight={2.4} />}>
          Everything between this phone and your desktop is end-to-end encrypted. The relay only ever sees ciphertext.
        </CardNote>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  page: { paddingHorizontal: 16, paddingTop: 22, paddingBottom: 40 },
  version: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  link: { flexDirection: 'row', alignItems: 'center', gap: 4 },
  figure: { ...shared.headLine, color: t.dim, fontVariant: ['tabular-nums'] },
});
