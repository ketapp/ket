import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { Alert } from 'react-native';

import { confirm } from './confirm';

describe('confirm', () => {
  beforeEach(() => {
    jest.restoreAllMocks();
  });

  test.each([
    [0, false],
    [1, true],
  ])('resolves native button %s', async (button, expected) => {
    jest.spyOn(Alert, 'alert').mockImplementation((_title, _message, buttons) => buttons?.[button]?.onPress?.());
    await expect(confirm('Delete?', 'Cannot undo.', 'Delete')).resolves.toBe(expected);
  });
});
