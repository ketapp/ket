import '../lib/polyfills';

import { FiraCode_400Regular, FiraCode_600SemiBold, useFonts } from '@expo-google-fonts/fira-code';
import { Geist_400Regular, Geist_500Medium, Geist_600SemiBold, Geist_900Black } from '@expo-google-fonts/geist';
import { DarkTheme, DefaultTheme, Stack, type Theme, ThemeProvider, usePathname } from 'expo-router';
import * as SplashScreen from 'expo-splash-screen';
import { StatusBar } from 'expo-status-bar';
import * as SystemUI from 'expo-system-ui';
import { View } from 'react-native';
import { KeyboardProvider } from 'react-native-keyboard-controller';

import { LockGate } from '../components/Lock';
import { Splash } from '../components/Splash';
import { WorktreeList } from '../components/WorktreeList';
import { useDesktopPalette, useHosts } from '../lib/hosts';
import { SIDEBAR_WIDTH, SplitContext, useWide } from '../lib/layout';
import { startLock, useLock } from '../lib/lock';
import { startNotices, useNoticeFollowing } from '../lib/notices';
import { light, t } from '../lib/theme';
import { installErrorCapture } from '../lib/crashes';

// Before anything else runs, so an error in what follows is kept too.
installErrorCapture();

// A screen that throws while drawing shows this rather than taking the app
// down; see components/ErrorScreen.
export { ErrorScreen as ErrorBoundary } from '../components/ErrorScreen';

SplashScreen.preventAutoHideAsync();
// The native splash goes at once, with no fade of its own: components/Splash
// has drawn the same frame over the app by then, and plays from it.
SplashScreen.setOptions({ duration: 0, fade: false });
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
    FiraCode_400Regular,
    FiraCode_600SemiBold,
  });
  const lock = useLock();
  // The desktop's theme, followed: a new one reloads the app to draw in it.
  useDesktopPalette();
  // News from the desktops, as banners while ket is not in front — see
  // lib/notices.ts. A tap opens its screen once the stack is drawn.
  useNoticeFollowing(loaded);
  // The sidebar, on a screen wide enough, once a desktop is paired: before
  // that there is no list to put in it. The paired desktops are read again
  // on every move, so pairing the first one brings the sidebar in.
  const path = usePathname();
  const paired = (useHosts(path)?.length ?? 0) > 0;
  const split = useWide() && paired;
  // Until the fonts are in, the native splash stays up; components/Splash
  // takes over from it.
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
      {/* The launch splash plays over all of it, and holds until the lock
          knows whether to show. */}
      <Splash ready={lock.ready}>
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
        {/* Over Worktrees, so a back gesture returns to the list. No slide:
            it opens from the bar, and the bar's other tabs swap in place. */}
        <Stack.Screen name="usage" options={{ animation: 'none' }} />
        <Stack.Screen name="pin" options={{ presentation: 'modal' }} />
        {/* The chats go back only from the screen's left edge. iOS 26 goes
            back on a swipe from anywhere, and that won over a table or code
            block scrolled sideways whenever its scroll couldn't start: with
            the keyboard up, or the chat still gliding, the chat's list holds
            the touch, to close the keyboard on a tap. */}
        <Stack.Screen name="session/[host]/[terminal]" options={{ fullScreenGestureEnabled: false }} />
        <Stack.Screen name="agent-session/[host]/[terminal]/[session]" options={{ fullScreenGestureEnabled: false }} />
        <Stack.Screen name="starting/[host]/[at]" options={{ fullScreenGestureEnabled: false }} />
        {/* A sheet over Worktrees, which stays drawn behind it. It animates
            itself in and out — see new/[host].tsx. */}
        <Stack.Screen
          name="new/[host]"
          options={{ presentation: 'transparentModal', animation: 'none', contentStyle: { backgroundColor: 'transparent' } }}
        />
        <Stack.Screen
          name="rename/[host]/[worktree]"
          options={{ presentation: 'transparentModal', animation: 'none', contentStyle: { backgroundColor: 'transparent' } }}
        />
      </Stack>
      </View>
      </View>
      </SplitContext.Provider>
      <LockGate />
      </Splash>
      </ThemeProvider>
    </KeyboardProvider>
  );
}
