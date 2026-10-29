import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render } from '@testing-library/react-native';

import { field, HORIZON, Horizon } from './Horizon';

const mockUseEffect = jest.requireActual<typeof import('react')>('react').useEffect;
jest.mock('expo-router', () => ({
  useFocusEffect: (callback: () => void) => mockUseEffect(callback, [callback]),
}));

describe('Horizon', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });
  afterEach(() => {
    jest.useRealTimers();
  });

  test('the planet rises from the bottom under a sky of stars', () => {
    const frame = field(390, 700, 0);
    const lit = (row: (readonly [string, number] | null)[]) => row.filter(Boolean).length;
    // The sky holds only the odd star; the planet's rim is dense.
    const sky = frame.slice(0, Math.floor(frame.length * 0.3));
    const ground = frame.slice(Math.ceil(frame.length * HORIZON) + 2);
    expect(Math.max(...sky.map(lit))).toBeLessThan(6);
    expect(Math.min(...ground.map(lit))).toBeGreaterThan(12);
    // The rim takes the brightest ink; the sky never does.
    expect(ground.flat().some((cell) => cell?.[1] === 0)).toBe(true);
    expect(sky.flat().every((cell) => cell === null || cell[1] > 0)).toBe(true);
    // Stars keep their places from one frame to the next; the grid moves.
    const later = field(390, 700, 3);
    const stars = (rows: typeof frame) => rows.map((row) => row.map((cell) => (cell ? 1 : 0)).join('')).join('\n');
    expect(stars(later.slice(0, 10))).toBe(stars(frame.slice(0, 10)));
    expect(stars(later)).not.toBe(stars(frame));
  });

  test('draws once it has a size, and ticks while on screen', async () => {
    const view = await render(<Horizon />);
    const picture = view.getByLabelText('A planet of glyphs under a starry sky');
    expect(picture.children).toHaveLength(0);
    await act(async () => {
      fireEvent(picture, 'layout', { nativeEvent: { layout: { x: 0, y: 0, width: 390, height: 700 } } });
    });
    const rows = () => view.getByLabelText('A planet of glyphs under a starry sky').children.length;
    expect(rows()).toBe(Math.ceil(700 / 12));
    const before = JSON.stringify(view.toJSON());
    await act(async () => {
      jest.advanceTimersByTime(1000);
    });
    expect(JSON.stringify(view.toJSON())).not.toBe(before);
    await view.unmount();
  });
});
