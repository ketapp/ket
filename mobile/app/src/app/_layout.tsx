import '../lib/polyfills';

import { Geist_400Regular, Geist_500Medium, Geist_600SemiBold, Geist_900Black } from '@expo-google-fonts/geist';
import { GeistMono_400Regular, GeistMono_500Medium, GeistMono_600SemiBold } from '@expo-google-fonts/geist-mono';
import { IBMPlexMono_400Regular, IBMPlexMono_600SemiBold, useFonts } from '@expo-google-fonts/ibm-plex-mono';
import { DarkTheme, DefaultTheme, Stack, type Theme, ThemeProvider, usePathname } from 'expo-router';
import * as SplashScreen from 'expo-splash-screen';
import { StatusBar } from 'expo-status-bar';
import * as SystemUI from 'expo-system-ui';
import { useEffect } from 'react';
import { View } from 'react-native';
import { KeyboardProvider } from 'react-native-keyboard-controller';

import { LockGate } from '../components/Lock';
import { WorktreeList } from '../components/WorktreeList';
import { useDesktopPalette, useHosts } from '../lib/hosts';
import { SIDEBAR_WIDTH, SplitContext, useWide } from '../lib/layout';
import { startLock, useLock } from '../lib/lock';
import { startNotices, useNoticeFollowing } from '../lib/notices';
import { light, t } from '../lib/theme';

SplashScreen.preventAutoHideAsync();
startLock();
startNotices();

// What shows between and behind screens — a screen sliding in, a sheet
// coming up, the window under them all — in the theme's backdrop. Left to
// itself navigation draws there in its own light theme, white and grey, and
// the window in app.json's fixed colour. A new theme reloads the app, so
// this is read once.
const base = light ? DefaultTheme : DarkTheme;
const navigation: Theme = {
  ...base,
  colors: { ...base.colors, background: t.backdrop, card: t.backdrop, text: t.text, border: t.border, primary: t.brand },
};
void SystemUI.setBackgroundColorAsync(t.backdrop);

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
  // News from the desktops, as banners while ket is not in front — see
  // lib/notices.ts.
  useNoticeFollowing();
  // The sidebar, on a screen wide enough, once a desktop is paired: before
  // that there is no list to put in it. The paired desktops are read again
  // on every move, so pairing the first one brings the sidebar in.
  const path = usePathname();
  const paired = (useHosts(path)?.length ?? 0) > 0;
  const split = useWide() && paired;
  useEffect(() => {
    if (loaded) void SplashScreen.hideAsync();
  }, [loaded]);
  if (!loaded) return null;
  // Behind the lock screen, the app is out of a screen reader's reach too.
  const hidden = !lock.ready || lock.locked || lock.confirming !== null;

  // Every screen draws its own bar. The four top-level tabs stay mounted
  // inside one stack screen, so returning keeps their layout and scroll.
  // The keyboard's own animation, frame for frame, for the screens that move
  // with it — see react-native-keyboard-controller.
  return (
    <KeyboardProvider>
      <ThemeProvider value={navigation}>
      <StatusBar style={light ? 'dark' : 'light'} />
      <SplitContext.Provider value={split}>
      <View
        style={{ flex: 1, flexDirection: 'row', backgroundColor: t.backdrop }}
        accessibilityElementsHidden={hidden}
        importantForAccessibility={hidden ? 'no-hide-descendants' : 'auto'}
      >
      {split ? (
        <View style={{ width: SIDEBAR_WIDTH }}>
          <WorktreeList sidebar refresh={path} />
        </View>
      ) : null}
      <View style={{ flex: 1 }}>
      <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: t.backdrop } }}>
        <Stack.Screen name="(tabs)" options={{ animation: 'none' }} />
        <Stack.Screen name="pin" options={{ presentation: 'modal' }} />
        {/* A sheet over Worktrees, which stays drawn behind it. It animates
            itself in and out — see new/[host].tsx. */}
        <Stack.Screen
          name="new/[host]"
          options={{ presentation: 'transparentModal', animation: 'none', contentStyle: { backgroundColor: 'transparent' } }}
        />
      </Stack>
      </View>
      </View>
      </SplitContext.Provider>
      <LockGate />
      </ThemeProvider>
    </KeyboardProvider>
  );
}
