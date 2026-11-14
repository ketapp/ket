import { describe, expect, test } from '@jest/globals';

import { sessionColor, t, tint } from './theme';

describe('tint', () => {
  test.each([
    ['#000000', 0, 'rgba(0,0,0,0)'],
    ['#ffffff', 1, 'rgba(255,255,255,1)'],
    ['#7aa7ff', 0.08, 'rgba(122,167,255,0.08)'],
  ])('converts %s at %s alpha', (hex, alpha, expected) => {
    expect(tint(hex, alpha)).toBe(expected);
  });
});

describe('sessionColor', () => {
  // WCAG contrast, worked out here rather than taken from the module, so
  // the test checks the rule rather than restating the code.
  const luminance = (hex: string) => {
    const n = parseInt(hex.slice(1), 16);
    const [r, g, b] = [16, 8, 0].map((shift) => {
      const c = ((n >> shift) & 255) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (a: string, b: string) => {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
  };

  test('is nothing for no colour, default, or one this build does not know', () => {
    expect(sessionColor(undefined)).toBeUndefined();
    expect(sessionColor('')).toBeUndefined();
    expect(sessionColor('default')).toBeUndefined();
    expect(sessionColor('magenta')).toBeUndefined();
  });

  test('paints red as the desktop does, with dark ink on its tile', () => {
    expect(sessionColor('red')).toEqual({ tile: '#ef4444', ink: t.card, label: '#ef4444' });
  });

  test.each(['red', 'blue', 'green', 'yellow', 'purple', 'orange', 'pink', 'cyan'])(
    '%s takes the ink that stands off its tile and a name that reads on a card',
    (name) => {
      const worn = sessionColor(name)!;
      const other = worn.ink === t.card ? t.text : t.card;
      expect([t.card, t.text]).toContain(worn.ink);
      expect(contrast(worn.ink, worn.tile)).toBeGreaterThanOrEqual(contrast(other, worn.tile));
      expect(contrast(worn.label, t.card)).toBeGreaterThanOrEqual(4.5);
    },
  );
});
