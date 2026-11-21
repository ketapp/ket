import { describe, expect, test } from '@jest/globals';
import { act } from '@testing-library/react-native';
import { renderRouter, screen, testRouter } from 'expo-router/testing-library';
import { router, Stack, Tabs } from 'expo-router';
import { Text } from 'react-native';

import { useEffect } from 'react';

import { showBeside, showOverWorktrees } from './lib/layout';

// Rows open their chat, terminal or worktree with `router.navigate` so a
// double tap opens it once. These hold the router to what that relies on.

const screens = {
  _layout: () => <Stack screenOptions={{ headerShown: false, animation: 'none' }} />,
  '(tabs)/_layout': () => <Stack screenOptions={{ headerShown: false }} />,
  '(tabs)/index': () => <Text>Worktrees</Text>,
  'session/[host]/[terminal]': () => <Text>Chat</Text>,
  'terminal/[host]/[terminal]': () => <Text>Terminal</Text>,
  'worktree/[host]/[worktree]': () => <Text>Worktree</Text>,
};

test.each(['/session/h/7', '/terminal/h/7', '/worktree/h/a%2Fb'])(
  'a row tapped twice opens %s once',
  async (href) => {
    await renderRouter(screens, { initialUrl: '/' });
    // Both taps land before the first has drawn.
    await act(async () => {
      router.navigate(href);
      router.navigate(href);
    });
    expect(testRouter.canGoBack()).toBe(true);
    await testRouter.back('/');
    expect(testRouter.canGoBack()).toBe(false);
  },
);

test('a chat further down is opened afresh, not moved up', async () => {
  await renderRouter(screens, { initialUrl: '/' });
  await act(async () => {
    router.navigate('/session/h/7');
  });
  await act(async () => {
    router.navigate('/worktree/h/w');
  });
  await act(async () => {
    router.navigate('/session/h/7');
  });
  await testRouter.back('/worktree/h/w');
  await testRouter.back('/session/h/7');
  await testRouter.back('/');
});

test('another chat still opens over the first', async () => {
  await renderRouter(screens, { initialUrl: '/' });
  await act(async () => {
    router.navigate('/session/h/7');
    router.navigate('/session/h/8');
  });
  await testRouter.back('/session/h/7');
  await testRouter.back('/');
});

// A tapped notice opens its screen with `showOverWorktrees`: Back from it
// goes to Worktrees, whatever the app had open when the notice came.
describe('a tapped notice', () => {
  // How many times Worktrees was drawn afresh: rebuilt, it loses its scroll.
  let built = 0;
  function Worktrees() {
    useEffect(() => {
      built++;
    }, []);
    return <Text>Worktrees</Text>;
  }
  const app = {
    _layout: () => <Stack screenOptions={{ headerShown: false, animation: 'none' }} />,
    '(tabs)/_layout': () => (
      <Tabs backBehavior="none" screenOptions={{ headerShown: false, animation: 'none' }} tabBar={() => null}>
        <Tabs.Screen name="index" />
        <Tabs.Screen name="settings" />
      </Tabs>
    ),
    '(tabs)/index': Worktrees,
    '(tabs)/settings': () => <Text>Settings</Text>,
    'session/[host]/[terminal]': () => <Text>Chat</Text>,
    'worktree/[host]/[worktree]': () => <Text>Worktree</Text>,
  };

  test('opens its chat over Worktrees, not over the chat that was open', async () => {
    await renderRouter(app, { initialUrl: '/' });
    await act(async () => {
      router.navigate('/worktree/h/w');
    });
    await act(async () => {
      router.navigate('/session/h/8');
    });
    await act(async () => {
      showOverWorktrees('/session/h/7');
    });
    expect(screen).toHavePathname('/session/h/7');
    await testRouter.back('/');
    expect(screen.getByText('Worktrees')).toBeTruthy();
    expect(testRouter.canGoBack()).toBe(false);
  });

  test('goes back to Worktrees from another tab too', async () => {
    await renderRouter(app, { initialUrl: '/settings' });
    await act(async () => {
      showOverWorktrees('/session/h/7');
    });
    await testRouter.back('/');
    expect(screen.getByText('Worktrees')).toBeTruthy();
  });

  test('opens over Worktrees when it is what started the app, and keeps it', async () => {
    built = 0;
    await renderRouter(app, { initialUrl: '/' });
    await act(async () => {
      showOverWorktrees('/session/h/7');
    });
    await testRouter.back('/');
    expect(testRouter.canGoBack()).toBe(false);
    expect(built).toBe(1);
  });

  test('beside the sidebar, takes the place of a chat picked there', async () => {
    await renderRouter(app, { initialUrl: '/' });
    // A sidebar pick replaces Worktrees: the chat is the stack's only screen.
    await act(async () => {
      showBeside('/session/h/8');
    });
    await act(async () => {
      showOverWorktrees('/session/h/7');
    });
    await testRouter.back('/');
    expect(screen.getByText('Worktrees')).toBeTruthy();
    expect(testRouter.canGoBack()).toBe(false);
  });
});
