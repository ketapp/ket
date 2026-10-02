// Pairing: scan the code the desktop shows, or paste its text.

import { CameraView, useCameraPermissions } from 'expo-camera';
import * as Device from 'expo-device';
import { router, useLocalSearchParams } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import { KeyboardAvoidingView, Platform, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { pair, toBase64Url } from '@ket/remote';

import { Ping } from '../components/Ping';
import { Bar, Button, Card, Heading, Prose, Screen, field } from '../components/ui';
import { phoneKey, saveHost } from '../lib/store';
import { font, size, t } from '../lib/theme';
import { versionLabel } from '../lib/version';

// A browser only lets a page use the camera over HTTPS; served from the desktop
// over plain HTTP, the code is pasted instead.
const noCamera = Platform.OS === 'web' && !globalThis.isSecureContext;

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
    const text = codeIn(scanned);
    if (busy.current || !text) return;
    busy.current = true;
    setState('pairing');
    try {
      const keys = await phoneKey();
      const { remote, host, confirmation: mine } = await pair(text, keys, {
        deviceName: Device.deviceName ?? (Platform.OS === 'web' ? 'Browser' : 'phone'),
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
          setConfirmation(mine);
          setState('confirm');
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
      router.replace(`/host/${id}`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
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
      <KeyboardAvoidingView style={styles.fill} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <Bar back title="Pair a desktop" />
        <ScrollView contentContainerStyle={styles.page} keyboardShouldPersistTaps="handled">
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
          ) : noCamera ? null : (
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
            <Step n={3}>{noCamera ? 'Copy the code it shows and paste it here' : 'Scan the code it shows'}</Step>
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
              autoCapitalize="none"
              autoCorrect={false}
            />
            <Button
              kind={noCamera ? 'primary' : 'secondary'}
              title={noCamera ? 'Pair' : 'Pair with a pasted code'}
              disabled={waiting || !code.trim()}
              onPress={() => void start(code)}
            />
          </Card>
        </ScrollView>
      </KeyboardAvoidingView>
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
  fill: { flex: 1 },
  page: { padding: size.gutter, paddingTop: 0, gap: size.gutter, paddingBottom: 32 },
  intro: { paddingHorizontal: 4 },
  finder: {
    height: 290,
    backgroundColor: t.sunken,
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
    borderWidth: 1,
    borderColor: t.border,
    borderRadius: size.radiusSm,
    overflow: 'hidden',
  },
  stepText: { flex: 1, fontFamily: font.prose, fontSize: size.body, color: t.text },
  form: { padding: size.pad, gap: 10 },
  status: { paddingVertical: 20, alignItems: 'center' },
  words: { alignItems: 'center', gap: 8 },
  title: { fontFamily: font.proseSemibold, fontSize: 20, color: t.text },
  meta: { fontFamily: font.chrome, fontSize: 12, color: t.dim },
  hint: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  confirmCode: { fontFamily: font.chromeMedium, fontSize: 30, letterSpacing: 2, color: t.text },
  code: { fontFamily: font.chrome, fontSize: size.label },
});

/**
 * The pairing code in what was scanned or pasted: the code itself, or the
 * web app's `…/pair?code=…` link the desktop's QR code carries when it knows
 * where the web app is served. `null` when it is neither.
 */
function codeIn(scanned: string): string | null {
  const text = scanned.trim();
  if (text.startsWith('ket://pair/')) return text;
  try {
    const code = new URL(text).searchParams.get('code');
    return code?.startsWith('ket://pair/') ? code : null;
  } catch {
    return null;
  }
}
