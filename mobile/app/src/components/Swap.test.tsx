import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, render } from '@testing-library/react-native';
import { Text } from 'react-native';
import { Fade, Wipe } from './Swap';

let mockStill = false;
jest.mock('react-native-reanimated', () => {
  const actual = jest.requireActual<typeof import('react-native-reanimated')>('react-native-reanimated');
  return { ...actual, __esModule: true, default: actual.default, useReducedMotion: () => mockStill };
});

const settle = async () => {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(600);
  });
};

const line = (word: string) => (
  <Wipe token={word} item={word} fill="#000">
    {(w) => <Text>{w}</Text>}
  </Wipe>
);
const state = (word: string) => (
  <Fade token={word} item={word}>
    {(w) => <Text>{w}</Text>}
  </Fade>
);

describe('Wipe', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    mockStill = false;
  });
  afterEach(() => {
    jest.useRealTimers();
  });

  test('holds the old words until the erase lands, then shows the new', async () => {
    const view = await render(line('Thinking'));
    await view.rerender(line('Editing'));
    // Mid-erase the line still says what it said.
    expect(view.getByText('Thinking')).toBeTruthy();
    expect(view.queryByText('Editing')).toBeNull();
    await settle();
    expect(view.getByText('Editing')).toBeTruthy();
    expect(view.queryByText('Thinking')).toBeNull();
  });

  test('takes up a change that lands mid-swap', async () => {
    const view = await render(line('a'));
    await view.rerender(line('b'));
    await view.rerender(line('c'));
    await settle();
    await settle();
    expect(view.getByText('c')).toBeTruthy();
    expect(view.queryByText('b')).toBeNull();
  });

  test('under Reduce Motion the new words are simply there', async () => {
    mockStill = true;
    const view = await render(line('Thinking'));
    await view.rerender(line('Editing'));
    expect(view.getByText('Editing')).toBeTruthy();
    expect(view.queryByText('Thinking')).toBeNull();
  });
});

describe('Fade', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    mockStill = false;
  });
  afterEach(() => {
    jest.useRealTimers();
  });

  test('shows the new word once the swap lands', async () => {
    const view = await render(state('running'));
    await view.rerender(state('waiting'));
    expect(view.getByText('running')).toBeTruthy();
    await settle();
    expect(view.getByText('waiting')).toBeTruthy();
  });

  test('under Reduce Motion it changes at once', async () => {
    mockStill = true;
    const view = await render(state('running'));
    await view.rerender(state('waiting'));
    expect(view.getByText('waiting')).toBeTruthy();
  });
});
