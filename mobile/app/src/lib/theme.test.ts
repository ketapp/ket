import { describe, expect, test } from '@jest/globals';

import { ramp, sessionColor, t, tint } from './theme';

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

describe('ramp', () => {
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
  // A theme's backdrop, surface and text, as the desktop ships them.
  const themes: Record<string, [string, string, string]> = {
    desk: ['#020304', '#0d0f12', '#f4f5f7'],
    light: ['#e4e7ec', '#fafbfc', '#111418'],
    'one-dark': ['#181a1f', '#282c34', '#dcdfe4'],
    'ms-dos': ['#000000', '#0000aa', '#55ffff'],
  };

  test('draws Desk in the greys that were picked', () => {
    expect(ramp(...themes.desk)).toEqual({
      backdrop: '#020304',
      card: '#121417',
      sunken: '#232528',
      elevated: '#232528',
      hover: '#232528',
      selection: '#36383b',
      border: '#46484b',
      rule: '#232528',
      faint: '#96989b',
      dim: '#bcbec0',
      text: '#f4f5f7',
    });
  });

  test.each(Object.keys(themes))('%s: each grey is a step further from the ground, and the inks read', (name) => {
    const greys = ramp(...themes[name]);
    const order = [greys.card, greys.sunken, greys.selection, greys.border, greys.faint, greys.dim, greys.text];
    const fromCard = order.map((grey) => contrast(grey, greys.card));
    expect(fromCard).toEqual([...fromCard].sort((a, b) => a - b));
    expect(contrast(greys.faint, greys.card)).toBeGreaterThanOrEqual(4.5);
    expect(contrast(greys.faint, greys.sunken)).toBeGreaterThanOrEqual(4.5);
    expect(contrast(greys.dim, greys.selection)).toBeGreaterThanOrEqual(4.5);
  });

  test('leaves a light theme its surface, whose ground is on the text side of it', () => {
    expect(ramp(...themes.light).card).toBe('#fafbfc');
  });
});
