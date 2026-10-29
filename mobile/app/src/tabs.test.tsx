import { expect, jest, test } from '@jest/globals';
import { fireEvent } from '@testing-library/react-native';
import type { JestNativeMatchers } from '@testing-library/react-native/dist/matchers/types';
import { renderRouter, testRouter } from 'expo-router/testing-library';
import { Stack } from 'expo-router';
import { useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';

import TabLayout from './app/(tabs)/_layout';
import { TabBar } from './components/ui';

// Router's test helpers augment `expect`; RNTL 14 augments `@jest/expect`.
// Keep the native matchers available when both helpers are loaded together.
declare module 'expect' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type -- Merges with Router's matcher interface.
  interface Matchers<R extends void | Promise<void>> extends JestNativeMatchers<R> {}
}

test('returning through the floating tabs keeps Worktrees mounted and its view in place', async () => {
  const mounted = jest.fn();
  const unmounted = jest.fn();
  function Worktrees() {
    const [folded, setFolded] = useState(false);
    useEffect(() => {
      mounted();
      return () => { unmounted(); };
    }, []);
    return (
      <View>
        <Pressable onPress={() => setFolded(!folded)} accessibilityRole="button" accessibilityLabel="Project">
          <Text>{folded ? 'Folded' : 'Expanded'}</Text>
        </Pressable>
        <TabBar active="worktrees" needs={0} />
      </View>
    );
  }
  const view = await renderRouter({
    _layout: () => <Stack screenOptions={{ headerShown: false, animation: 'none' }} />,
    '(tabs)/_layout': TabLayout,
    '(tabs)/index': Worktrees,
    '(tabs)/usage': () => <TabBar active="usage" needs={0} />,
    '(tabs)/needs': () => <TabBar active="needs" needs={0} />,
    '(tabs)/settings': () => <TabBar active="settings" needs={0} />,
    detail: () => <Text>Worktree detail</Text>,
  }, { initialUrl: '/' });

  await fireEvent.press(view.getByLabelText('Project'));
  for (const [label, path] of [['Usage', '/usage'], ['Needs you', '/needs'], ['Settings', '/settings']]) {
    await fireEvent.press(view.getByLabelText(label));
    expect(view.getPathname()).toBe(path);
    expect(testRouter.canGoBack()).toBe(false);
    await fireEvent.press(view.getByLabelText('Worktrees'));
    expect(view.getPathname()).toBe('/');
    expect(view.getByText('Folded')).toBeTruthy();
    expect(mounted).toHaveBeenCalledTimes(1);
    expect(unmounted).not.toHaveBeenCalled();
  }

  // Worktree details still push over the tabs and return to the same view.
  await testRouter.push('/detail');
  await testRouter.back('/');
  expect(view.getByText('Folded')).toBeTruthy();
  expect(mounted).toHaveBeenCalledTimes(1);
  expect(unmounted).not.toHaveBeenCalled();
});
