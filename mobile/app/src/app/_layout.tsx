import '../lib/polyfills';

import { Geist_400Regular, Geist_500Medium, Geist_600SemiBold, Geist_900Black } from '@expo-google-fonts/geist';
import { GeistMono_400Regular, GeistMono_500Medium, GeistMono_600SemiBold } from '@expo-google-fonts/geist-mono';
import { IBMPlexMono_400Regular, IBMPlexMono_600SemiBold, useFonts } from '@expo-google-fonts/ibm-plex-mono';
import { Stack } from 'expo-router';
import * as SplashScreen from 'expo-splash-screen';
import { StatusBar } from 'expo-status-bar';
import { useEffect } from 'react';
import { View } from 'react-native';
import { KeyboardProvider } from 'react-native-keyboard-controller';

import { LockGate } from '../components/Lock';
import { useDesktopPalette } from '../lib/hosts';
import { startLock, useLock } from '../lib/lock';
import { light, t } from '../lib/theme';

SplashScreen.preventAutoHideAsync();
startLock();

export default function Layout() {
  const [loaded] = useFonts({
    Geist_400Regular,
    Geist_500Medium,
    Geist_600SemiBold,
    Geist_900Black,
    GeistMono_400Regular,
    GeistMono_500Medium,
    GeistMono_600SemiBold,
    IBMPlexMono_400Regular,
    IBMPlexMono_600SemiBold,
  });
  const lock = useLock();
  // The desktop's theme, followed: a new one reloads the app to draw in it.
  useDesktopPalette();
  useEffect(() => {
    if (loaded) void SplashScreen.hideAsync();
  }, [loaded]);
  if (!loaded) return null;
  // Behind the lock screen, the app is out of a screen reader's reach too.
  const hidden = !lock.ready || lock.locked || lock.confirming !== null;

  // Every screen draws its own bar. The three the tab bar switches between
  // swap in place rather than sliding: they are one place, not a path.
  // The keyboard's own animation, frame for frame, for the screens that move
  // with it — see react-native-keyboard-controller.
  return (
    <KeyboardProvider>
      <StatusBar style={light ? 'dark' : 'light'} />
      <View
        style={{ flex: 1, backgroundColor: t.backdrop }}
        accessibilityElementsHidden={hidden}
        importantForAccessibility={hidden ? 'no-hide-descendants' : 'auto'}
      >
      <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: t.backdrop } }}>
        <Stack.Screen name="index" options={{ animation: 'none' }} />
        <Stack.Screen name="needs" options={{ animation: 'none' }} />
        <Stack.Screen name="usage" options={{ animation: 'none' }} />
        <Stack.Screen name="settings" options={{ animation: 'none' }} />
        <Stack.Screen name="pin" options={{ presentation: 'modal' }} />
        {/* A sheet over Worktrees, which stays drawn behind it. It animates
            itself in and out — see new/[host].tsx. */}
        <Stack.Screen
          name="new/[host]"
          options={{ presentation: 'transparentModal', animation: 'none', contentStyle: { backgroundColor: 'transparent' } }}
        />
      </Stack>
      </View>
      <LockGate />
    </KeyboardProvider>
  );
}
