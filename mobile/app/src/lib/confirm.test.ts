import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { Alert, Platform } from 'react-native';

import { confirm } from './confirm';

describe('confirm', () => {
  beforeEach(() => {
    jest.restoreAllMocks();
  });

  test('uses browser confirm on web', async () => {
    Object.defineProperty(Platform, 'OS', { configurable: true, value: 'web' });
    const browser = jest.fn(() => true);
    Object.defineProperty(globalThis, 'confirm', { configurable: true, value: browser });
    await expect(confirm('Forget?', 'This removes it.', 'Forget')).resolves.toBe(true);
    expect(browser).toHaveBeenCalledWith('Forget?\n\nThis removes it.');
  });

  test.each([
    [0, false],
    [1, true],
  ])('resolves native button %s', async (button, expected) => {
    Object.defineProperty(Platform, 'OS', { configurable: true, value: 'ios' });
    jest.spyOn(Alert, 'alert').mockImplementation((_title, _message, buttons) => buttons?.[button]?.onPress?.());
    await expect(confirm('Delete?', 'Cannot undo.', 'Delete')).resolves.toBe(expected);
  });
});
