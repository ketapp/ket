// Choosing the app lock's PIN: once, then again to be sure. Turning the lock
// on comes here, and so does Change PIN — the PIN is the way in whenever
// Face ID or a fingerprint is not. See lib/lock.ts.

import { router } from 'expo-router';
import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { PinPad } from '../components/Lock';
import { PIN_LENGTH, setPin, useLock } from '../lib/lock';
import { font, size, t } from '../lib/theme';

export default function ChoosePin() {
  const lock = useLock();
  const insets = useSafeAreaInsets();
  const [first, setFirst] = useState<string | null>(null);
  const [error, setError] = useState('');

  const entered = async (pin: string) => {
    if (first === null) {
      setFirst(pin);
      setError('');
      return;
    }
    if (pin !== first) {
      setFirst(null);
      setError("The PINs didn't match · start again");
      return;
    }
    await setPin(pin);
    router.back();
  };

  return (
    <View style={[styles.page, { paddingTop: insets.top + 12, paddingBottom: Math.max(insets.bottom, 16) }]}>
      <View style={styles.top}>
        <Pressable accessibilityRole="button" onPress={() => router.back()} hitSlop={10}>
          <Text style={styles.link}>Cancel</Text>
        </Pressable>
      </View>
      <PinPad
        key={first === null ? 'first' : 'again'}
        title={first === null ? (lock.on ? 'Choose a new PIN' : 'Choose a PIN') : 'Enter it again'}
        detail={
          first === null
            ? `${PIN_LENGTH} digits. It unlocks ket when Face ID or your fingerprint can't.`
            : 'To be sure it is the one you meant.'
        }
        error={error || undefined}
        onComplete={(pin) => void entered(pin)}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, backgroundColor: t.backdrop },
  top: { height: 32, paddingHorizontal: size.gutter + 10, justifyContent: 'center' },
  link: { fontFamily: font.proseMedium, fontSize: size.body, color: t.dim },
});
