// What the app lock draws — see lib/lock.ts: the PIN pad, and the gate over
// every screen that is the lock screen, the "confirm it's you" sheet, or a
// plain cover while the app is out of sight.

import { router } from 'expo-router';
import { type ReactNode, useEffect, useRef, useState } from 'react';
import { Alert, AppState, Pressable, StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  PIN_LENGTH,
  biometry,
  cancelConfirm,
  forgetEverything,
  unlockWithBiometrics,
  unlockWithPin,
  useLock,
  waitMs,
} from '../lib/lock';
import { font, size, t } from '../lib/theme';
import { BrandMark, Erase, FaceScan, Fingerprint } from './icons';

/** The icon for a way of unlocking, by its name from `biometry()`. */
function biometricIcon(kind: string, color: string) {
  return kind === 'Face ID' || kind === 'Face unlock' ? (
    <FaceScan size={26} color={color} />
  ) : (
    <Fingerprint size={26} color={color} />
  );
}

/**
 * Six dots and a keypad. Calls `onComplete` with the PIN once the last digit
 * is in, and clears itself. The bottom-left key is `extra` — biometrics on
 * the lock screen — or blank.
 */
export function PinPad({
  title,
  detail,
  error,
  disabled,
  extra,
  onComplete,
}: {
  title: string;
  detail?: string;
  error?: string;
  disabled?: boolean;
  extra?: ReactNode;
  onComplete: (pin: string) => void;
}) {
  const [digits, setDigits] = useState('');
  const press = (digit: string) => {
    if (disabled) return;
    const next = (digits + digit).slice(0, PIN_LENGTH);
    setDigits(next);
    if (next.length === PIN_LENGTH) {
      // Drawn full for a moment, then handed on.
      setTimeout(() => {
        setDigits('');
        onComplete(next);
      }, 90);
    }
  };
  const keys = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];
  return (
    <View style={styles.pad}>
      <View style={styles.heading}>
        <Text style={styles.title}>{title}</Text>
        {detail ? <Text style={styles.detail}>{detail}</Text> : null}
      </View>
      <View style={styles.dots} accessibilityLabel={`${digits.length} of ${PIN_LENGTH} digits`}>
        {Array.from({ length: PIN_LENGTH }, (_, at) => (
          <View key={at} style={[styles.dot, at < digits.length && styles.dotOn, error ? styles.dotError : null]} />
        ))}
      </View>
      <Text style={styles.error}>{error ?? ' '}</Text>
      <View style={styles.keys}>
        {keys.map((key) => (
          <Key key={key} well label={key} onPress={() => press(key)} disabled={disabled}>
            <Text style={styles.keyText}>{key}</Text>
          </Key>
        ))}
        <View style={styles.key}>{extra}</View>
        <Key well label="0" onPress={() => press('0')} disabled={disabled}>
          <Text style={styles.keyText}>0</Text>
        </Key>
        <Key label="Delete" onPress={() => setDigits((d) => d.slice(0, -1))} disabled={disabled || !digits}>
          <Erase size={24} color={digits ? t.text : t.faint} />
        </Key>
      </View>
    </View>
  );
}

function Key({
  label,
  onPress,
  disabled,
  well,
  children,
}: {
  label: string;
  onPress: () => void;
  disabled?: boolean;
  /** A digit, set in a well; the keys around them are bare. */
  well?: boolean;
  children: ReactNode;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      disabled={disabled}
      style={({ pressed }) => [
        styles.key,
        well && { backgroundColor: t.sunken },
        pressed && { backgroundColor: t.selection },
        disabled && { opacity: 0.4 },
      ]}
    >
      {children}
    </Pressable>
  );
}

/** Minutes and seconds: 0:30. */
function clock(ms: number): string {
  const seconds = Math.ceil(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

/** Over every screen: the lock screen, the confirm sheet, or the cover. */
export function LockGate() {
  const lock = useLock();
  const insets = useSafeAreaInsets();
  const [kind, setKind] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [wait, setWait] = useState(0);
  const asked = useRef(false);
  const showing = lock.locked || lock.confirming !== null;

  useEffect(() => {
    void biometry().then(setKind);
  }, [lock.on]);

  // Biometrics are asked for as the lock screen comes up — once per
  // showing, and only with the app in front, where the prompt can appear.
  useEffect(() => {
    if (!lock.locked) {
      asked.current = false;
      return;
    }
    const ask = () => {
      if (asked.current || !kind || AppState.currentState !== 'active') return;
      asked.current = true;
      void unlockWithBiometrics();
    };
    ask();
    const sub = AppState.addEventListener('change', (next) => {
      if (next === 'active') ask();
    });
    return () => sub.remove();
  }, [lock.locked, kind]);

  // The wait after too many wrong PINs, counted down.
  useEffect(() => {
    if (!showing) return;
    const timer = setInterval(() => setWait(waitMs()), 500);
    return () => clearInterval(timer);
  }, [showing]);

  if (!lock.ready) return <View style={styles.cover} />;
  if (!showing) {
    return lock.covered ? (
      <View style={[styles.cover, styles.center]}>
        <BrandMark size={34} />
      </View>
    ) : null;
  }

  const confirming = lock.confirming !== null;
  const submit = async (pin: string) => {
    const result = await unlockWithPin(pin);
    if (result.ok) {
      setError('');
      return;
    }
    setWait(result.waitMs);
    setError(
      result.waitMs > 0 ? 'Too many wrong PINs' : result.left === 1 ? 'Wrong PIN · 1 try before a wait' : 'Wrong PIN',
    );
  };
  const forgot = () =>
    Alert.alert(
      'Forgot your PIN?',
      'ket will forget every paired desktop and turn the lock off. Pair your desktops again to use them.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Forget desktops',
          style: 'destructive',
          onPress: () => {
            void forgetEverything().then(() => router.replace('/'));
          },
        },
      ],
    );

  return (
    <View
      style={[styles.cover, { paddingTop: insets.top + 12, paddingBottom: Math.max(insets.bottom, 16) }]}
      accessibilityViewIsModal
    >
      <View style={styles.top}>
        {confirming ? (
          <Pressable accessibilityRole="button" onPress={cancelConfirm} hitSlop={10}>
            <Text style={styles.link}>Cancel</Text>
          </Pressable>
        ) : (
          <BrandMark size={22} />
        )}
      </View>
      <PinPad
        title={confirming ? "Confirm it's you" : 'ket is locked'}
        detail={confirming ? (lock.confirming ?? undefined) : `Enter your PIN${kind ? ` or use ${kind}` : ''}`}
        error={wait > 0 ? `Try again in ${clock(wait)}` : error || undefined}
        disabled={wait > 0}
        extra={
          kind ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={`Use ${kind}`}
              onPress={() => void unlockWithBiometrics()}
              style={({ pressed }) => [styles.key, pressed && { backgroundColor: t.selection }]}
            >
              {biometricIcon(kind, t.text)}
            </Pressable>
          ) : null
        }
        onComplete={(pin) => void submit(pin)}
      />
      <Pressable accessibilityRole="button" onPress={forgot} hitSlop={10} style={styles.forgot}>
        <Text style={styles.link}>Forgot PIN?</Text>
      </Pressable>
    </View>
  );
}

const KEY = 72;

const styles = StyleSheet.create({
  cover: { ...StyleSheet.absoluteFill, backgroundColor: t.backdrop, zIndex: 100 },
  center: { alignItems: 'center', justifyContent: 'center' },
  top: { height: 32, paddingHorizontal: size.gutter + 10, justifyContent: 'center', alignItems: 'flex-start' },
  link: { fontFamily: font.proseMedium, fontSize: size.body, color: t.dim },
  forgot: { alignSelf: 'center', paddingVertical: 8 },
  pad: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: 22 },
  heading: { alignItems: 'center', gap: 6, paddingHorizontal: 24 },
  title: { fontFamily: font.proseSemibold, fontSize: 20, color: t.text },
  detail: { fontFamily: font.prose, fontSize: size.body, color: t.dim, textAlign: 'center', lineHeight: 20 },
  dots: { flexDirection: 'row', gap: 14 },
  dot: { width: 12, height: 12, borderRadius: 6, borderWidth: 1.5, borderColor: t.faint },
  dotOn: { backgroundColor: t.text, borderColor: t.text },
  dotError: { borderColor: t.failed },
  error: { fontFamily: font.chrome, fontSize: size.detail, color: t.failed, marginTop: -8 },
  keys: { width: KEY * 3 + 28 * 2, flexDirection: 'row', flexWrap: 'wrap', columnGap: 28, rowGap: 14 },
  key: { width: KEY, height: KEY, borderRadius: KEY / 2, alignItems: 'center', justifyContent: 'center' },
  keyText: { fontFamily: font.chrome, fontSize: 26, color: t.text },
});
