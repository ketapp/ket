import { expect, test } from '@jest/globals';
import { act } from '@testing-library/react-native';
import { renderRouter, testRouter } from 'expo-router/testing-library';
import { router, Stack } from 'expo-router';
import { Text } from 'react-native';

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
