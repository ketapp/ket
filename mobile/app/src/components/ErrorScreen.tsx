// What a screen that failed to draw shows instead of the app vanishing: what
// happened, a way to try again, and the report to send. The root layout
// exports it as its error boundary, so it stands in for any screen under it.

import type { ErrorBoundaryProps } from 'expo-router';
import * as SplashScreen from 'expo-splash-screen';
import { useEffect } from 'react';
import { Share, StyleSheet, Text, View } from 'react-native';
import { readReports, recordError, reportText } from '../lib/crashes';
import { font, size, t } from '../lib/theme';
import { Button, Card, Heading, Prose, Screen } from './ui';

export function ErrorScreen({ error, retry }: ErrorBoundaryProps) {
  useEffect(() => {
    recordError(error, { fatal: false, where: 'screen' });
    // An error before the first screen drew would leave the splash over this.
    void SplashScreen.hideAsync();
  }, [error]);

  const share = () => void Share.share({ message: reportText(readReports()) });

  return (
    <Screen>
      <View style={styles.page}>
        <Card style={styles.card}>
          <Heading>Something went wrong</Heading>
          <Prose style={styles.prose}>
            {"This screen hit an error and couldn't be drawn. Try again, or share the report so it can be fixed. It stays on this phone unless you share it."}
          </Prose>
          <Text style={styles.message} selectable numberOfLines={4}>
            {error.message}
          </Text>
          <View style={styles.actions}>
            <Button kind="primary" title="Try again" onPress={() => void retry()} />
            <Button kind="secondary" title="Share report" onPress={share} />
          </View>
        </Card>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, justifyContent: 'center', padding: size.gutter },
  card: { padding: 14, gap: 10 },
  prose: { fontSize: size.body, lineHeight: 20 },
  message: { fontFamily: font.code, fontSize: size.detail, lineHeight: 18, color: t.dim },
  actions: { flexDirection: 'row', gap: 8, paddingTop: 4 },
});
