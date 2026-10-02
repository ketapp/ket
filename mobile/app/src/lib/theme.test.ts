import { describe, expect, test } from '@jest/globals';

import { tint } from './theme';

describe('tint', () => {
  test.each([
    ['#000000', 0, 'rgba(0,0,0,0)'],
    ['#ffffff', 1, 'rgba(255,255,255,1)'],
    ['#7aa7ff', 0.08, 'rgba(122,167,255,0.08)'],
  ])('converts %s at %s alpha', (hex, alpha, expected) => {
    expect(tint(hex, alpha)).toBe(expected);
  });
});
