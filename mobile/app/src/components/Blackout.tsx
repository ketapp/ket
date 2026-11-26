// What a blackout draws — see lib/blackout.ts: an empty sky over every
// screen, saying when ket comes back and that only the desktop can let the
// phone in sooner. Nothing on it can be pressed: there is no way round a
// blackout from the phone, and the cover says so by offering none.
//
// The sky is the Needs you tab's, without its planet (components/Horizon),
// and the words stand in it the way that tab's do.

import { useEffect } from 'react';
import { Keyboard, StyleSheet, View } from 'react-native';

import { clockOf, setCovered, useBlackout } from '../lib/blackout';
import { size, t } from '../lib/theme';
import { StarField } from './Horizon';
import { Prose } from './ui';

/** Over every screen, under the app lock: the cover while a blackout is on,
 * and a plain one until the kept hours have been read. */
export function BlackoutGate() {
  const blackout = useBlackout();
  const shut = blackout.shut !== null;

  // A keyboard left up would stand over the cover, typing into what is
  // under it; and so would a sheet or a menu, which close on this word.
  useEffect(() => {
    if (shut) Keyboard.dismiss();
    setCovered(shut);
    return () => setCovered(false);
  }, [shut]);

  if (!blackout.ready) return <View style={styles.cover} />;
  if (!blackout.shut) return null;
  return (
    <View style={styles.cover} accessibilityViewIsModal>
      <StarField />
      <View style={styles.words}>
        <Prose color={t.text} style={styles.title}>
          {`ket is off until ${clockOf(blackout.shut.until)}.`}
        </Prose>
        <Prose style={styles.note}>
          {`Blackout hours are set on ${blackout.shut.name}. Only that Mac can let this phone back in. Your agents keep working.`}
        </Prose>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  // One under the app lock's cover, which asks for the PIN first.
  cover: { ...StyleSheet.absoluteFill, backgroundColor: t.backdrop, zIndex: 99 },
  // The words sit centred in the sky.
  words: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
    gap: 6,
  },
  title: { fontSize: size.title, textAlign: 'center' },
  note: { textAlign: 'center' },
});
