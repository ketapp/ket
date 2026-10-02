import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render, waitFor } from '@testing-library/react-native';
import { AccessibilityInfo } from 'react-native';
import { Unpaired } from './Unpaired';

const mockPush = jest.fn();
jest.mock('expo-router', () => ({ router: { push: (path: string) => mockPush(path) } }));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));

describe('Unpaired', () => {
  beforeEach(() => {
    jest.spyOn(AccessibilityInfo, 'isReduceMotionEnabled').mockResolvedValue(true);
  });

  test('shows pairing steps and both pairing routes', async () => {
    const view = await render(<Unpaired />);
    const { getByText, getByLabelText } = view;
    expect(getByLabelText('Your agents, in your pocket.')).toBeTruthy();
    for (const text of ['Open ket on your desktop', 'Scan the code it shows', 'Follow every run and answer what it asks.']) {
      expect(getByText(text)).toBeTruthy();
    }
    await fireEvent.press(getByText('Scan the code'));
    await fireEvent.press(getByText('Paste a code instead'));
    expect(mockPush.mock.calls).toEqual([['/pair'], ['/pair?paste=1']]);
    await waitFor(() => expect(AccessibilityInfo.isReduceMotionEnabled).toHaveBeenCalled());
    await view.unmount();
  });
});
