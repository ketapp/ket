// Pairing: scan the code the desktop shows, or paste its text.

import { CameraView, useCameraPermissions } from 'expo-camera';
import * as Device from 'expo-device';
import { router, useLocalSearchParams } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import { Keyboard, StyleSheet, Text, TextInput, View } from 'react-native';
import { KeyboardAwareScrollView } from 'react-native-keyboard-controller';
import { pair, toBase64Url } from '@ket/remote';

import { Ping } from '../components/Ping';
import { Bar, Button, Card, Heading, Prose, Screen, field, oneLine } from '../components/ui';
import { reachIn } from '../lib/addresses';
import { dropConnection } from '../lib/connection';
import { phoneKey, saveHost } from '../lib/store';
import { reasonOf } from '../lib/text';
import { font, size, t } from '../lib/theme';
import { surface } from '../components/ui/surface';
import { versionLabel } from '../lib/version';

const FORM_GAP = 10;

/** Pairs with the desktop whose code is `text`: the handshake, then the wait
 * while a person approves it there, told to `confirm` with the six digits to
 * compare. The new pairing's id once the desktop has welcomed this phone.
 * Out here because the React Compiler skips a component with a loop like
 * this one, or a throw, inside a try. */
async function pairWith(text: string, confirm: (code: string) => void): Promise<string> {
  const keys = await phoneKey();
  const { remote, host, confirmation: mine } = await pair(text, keys, {
    deviceName: Device.deviceName ?? 'phone',
    appVersion: versionLabel,
  });
  let name = 'Desktop';
  // A desktop asks a person to approve this phone first; the wait is as
  // long as it says it will wait, and a little more.
  let waitMs = 15_000;
  for (;;) {
    const message = await remote.recv(waitMs);
    if (message.payload.case === 'pairingPending') {
      // Worked out on this phone from the handshake; the desktop's copy
      // must agree, or this is not the pairing the desktop is showing.
      if (message.payload.value.code !== mine) {
        remote.close();
        throw new Error('The desktop and this phone disagree about the pairing code. Try again.');
      }
      confirm(mine);
      waitMs = (message.payload.value.timeoutSeconds + 15) * 1000;
      continue;
    }
    if (message.payload.case === 'welcome') {
      name = message.payload.value.hostName || name;
      break;
    }
    if (message.payload.case === 'error') {
      remote.close();
      throw new Error(message.payload.value.message);
    }
  }
  remote.close();
  const id = toBase64Url(host.hostId);
  await saveHost({
    id,
    name,
    relay: host.relay,
    hostPublic: toBase64Url(host.hostPublic),
    pairedAt: Date.now(),
  });
  // Pairing again with a desktop this phone already knew: the open
  // connection still holds the old pairing and would keep dialing it
  // until the app restarted. The next screen builds one from this.
  dropConnection(id);
  return id;
}

export default function Pair() {
  // A code can also arrive in a link — ket://pair?code=… — which pairs at once.
  // Or the first screen's "Paste a code instead", which opens on the field.
  const { code: linked, paste } = useLocalSearchParams<{ code?: string; paste?: string }>();
  const [permission, requestPermission] = useCameraPermissions();
  const [code, setCode] = useState(linked ?? '');
  const [state, setState] = useState<'idle' | 'pairing' | 'confirm' | 'failed'>('idle');
  const [error, setError] = useState('');
  // The six digits to compare with the desktop's, while it decides.
  const [confirmation, setConfirmation] = useState('');
  const busy = useRef(false);
  // From the scan until the desktop answers: the camera gives way to what is
  // happening, at the top where the eye already is.
  const waiting = state === 'pairing' || state === 'confirm';

  const start = async (scanned: string) => {
    // A desktop's reach code, scanned here rather than with the Camera: it
    // adds to a pairing this phone has, and needs no handshake.
    const reach = reachIn(scanned);
    if (reach && !busy.current) {
      busy.current = true;
      router.replace(`/reach?host=${reach.hostId}&at=${encodeURIComponent(reach.addresses.join(' '))}`);
      return;
    }
    const text = scanned.trim();
    if (busy.current || !text.startsWith('ket://pair/')) return;
    busy.current = true;
    Keyboard.dismiss();
    setState('pairing');
    try {
      const id = await pairWith(text, (mine) => {
        setConfirmation(mine);
        setState('confirm');
      });
      router.replace(`/host/${id}`);
    } catch (e) {
      setError(reasonOf(e));
      setState('failed');
      busy.current = false;
    }
  };

  // Once, for the code the screen was opened with. Starting a handshake is
  // what an effect is for; the state `start` sets only records that it began.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    if (linked) void start(linked);
  }, [linked]);

  return (
    <Screen>
      <Bar back title="Pair a desktop" />
      <KeyboardAwareScrollView
        contentContainerStyle={styles.page}
        keyboardShouldPersistTaps="handled"
        // The offset starts at the caret. Leave room for the rest of the
        // field, the Pair button below it, and the card's bottom padding.
        bottomOffset={2 * size.control + FORM_GAP + size.pad}
      >
        <Prose style={styles.intro}>
          The phone reaches your desktop through an encrypted relay. Nothing leaves the desktop in the clear.
        </Prose>

        {waiting ? (
          <Card style={styles.status}>
            <View accessibilityLiveRegion="polite" style={styles.words}>
              <Ping stuck={false} />
              {state === 'pairing' ? (
                <>
                  <Text style={styles.title}>Reaching your desktop…</Text>
                  <Text style={styles.meta}>Checking the code with ket</Text>
                </>
              ) : (
                <>
                  <Text style={styles.title}>Approve on your desktop</Text>
                  <Text style={styles.confirmCode}>{confirmation}</Text>
                  <Text style={styles.meta}>Waiting for your desktop · it shows the same code</Text>
                  <Text style={styles.hint}>If the codes differ, decline it there.</Text>
                </>
              )}
            </View>
          </Card>
        ) : (
          <Card style={styles.finder}>
            {permission?.granted ? (
              <CameraView
                style={StyleSheet.absoluteFill}
                barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
                onBarcodeScanned={(result) => void start(result.data)}
              />
            ) : (
              <Button title="Allow the camera" onPress={() => void requestPermission()} />
            )}
            <View pointerEvents="none" style={styles.frame}>
              <View style={[styles.corner, { top: 0, left: 0, borderTopWidth: 2, borderLeftWidth: 2, borderTopLeftRadius: 8 }]} />
              <View style={[styles.corner, { top: 0, right: 0, borderTopWidth: 2, borderRightWidth: 2, borderTopRightRadius: 8 }]} />
              <View style={[styles.corner, { bottom: 0, left: 0, borderBottomWidth: 2, borderLeftWidth: 2, borderBottomLeftRadius: 8 }]} />
              <View style={[styles.corner, { bottom: 0, right: 0, borderBottomWidth: 2, borderRightWidth: 2, borderBottomRightRadius: 8 }]} />
            </View>
            <Text style={styles.finderText}>Point at the code in ket</Text>
          </Card>
        )}

        <Card style={styles.card}>
          <Heading>On your desktop</Heading>
          <Step n={1}>Open ket</Step>
          <Step n={2}>
            Settings <Text style={{ color: t.faint }}>›</Text> Devices
          </Step>
          <Step n={3}>Scan the code it shows</Step>
        </Card>

        <Card style={styles.form}>
          {state === 'failed' ? <Prose color={t.failed}>{error}</Prose> : null}
          <TextInput
            style={[field, styles.code]}
            value={code}
            onChangeText={setCode}
            placeholder="ket://pair/…"
            autoFocus={paste === '1'}
            placeholderTextColor={t.faint}
            {...oneLine}
            autoCapitalize="none"
            autoCorrect={false}
          />
          <Button
            kind="secondary"
            title="Pair with a pasted code"
            disabled={waiting || !code.trim()}
            onPress={() => void start(code)}
          />
        </Card>
      </KeyboardAwareScrollView>
    </Screen>
  );
}

function Step({ n, children }: { n: number; children: React.ReactNode }) {
  return (
    <View style={styles.step}>
      <Text style={styles.kbd}>{n}</Text>
      <Text style={styles.stepText}>{children}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  page: { padding: size.gutter, paddingTop: 0, gap: size.gutter, paddingBottom: 32 },
  intro: { paddingHorizontal: 4 },
  finder: {
    height: 290,
    ...surface.well,
    alignItems: 'center',
    justifyContent: 'center',
  },
  frame: { position: 'absolute', width: 196, height: 196 },
  corner: { position: 'absolute', width: 30, height: 30, borderColor: t.accent },
  finderText: {
    position: 'absolute',
    bottom: 14,
    fontFamily: font.prose,
    fontSize: size.detail,
    color: t.dim,
  },
  card: { padding: 8, gap: 2 },
  step: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: 12, paddingHorizontal: 8 },
  kbd: {
    minWidth: 22,
    height: 20,
    lineHeight: 18,
    textAlign: 'center',
    fontFamily: font.chrome,
    fontSize: 11,
    color: t.dim,
    ...surface.outline,
    borderRadius: size.radiusSm,
    overflow: 'hidden',
  },
  stepText: { flex: 1, fontFamily: font.prose, fontSize: size.body, color: t.text },
  form: { padding: size.pad, gap: FORM_GAP },
  status: { paddingVertical: 20, alignItems: 'center' },
  words: { alignItems: 'center', gap: 8 },
  title: { fontFamily: font.proseSemibold, fontSize: 20, color: t.text },
  meta: { fontFamily: font.chrome, fontSize: 12, color: t.dim },
  hint: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  confirmCode: { fontFamily: font.chromeMedium, fontSize: 30, letterSpacing: 2, color: t.text },
  code: { fontFamily: font.chrome, fontSize: size.label },
});
