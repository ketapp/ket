import { describe, expect, it } from '@jest/globals';
import { AutomationSummarySchema, create } from '@ket/remote';

import { rowDetail, startLabel } from './AutomationsSheet';

describe('the Automations sheet', () => {
  const now = new Date(2026, 10, 19, 15, 0).getTime();

  it('says a start by its day: today, tomorrow, or the date', () => {
    expect(startLabel(new Date(2026, 10, 19, 18, 0).getTime(), now)).toMatch(/^Today /);
    expect(startLabel(new Date(2026, 10, 20, 9, 0).getTime(), now)).toMatch(/^Tomorrow /);
    expect(startLabel(new Date(2026, 10, 23, 9, 0).getTime(), now)).not.toMatch(/^(Today|Tomorrow)/);
  });

  it('says when a row starts next, or that it is paused or done', () => {
    const next = BigInt(new Date(2026, 10, 20, 9, 0).getTime());
    const base = { schedule: 'Weekdays at 9:00 AM' };
    expect(rowDetail(create(AutomationSummarySchema, { ...base, enabled: true, nextStartMs: next }), now)).toMatch(
      /^Next Tomorrow .* · Weekdays at 9:00 AM$/,
    );
    expect(rowDetail(create(AutomationSummarySchema, { ...base, enabled: false }), now)).toBe('Paused · Weekdays at 9:00 AM');
    expect(rowDetail(create(AutomationSummarySchema, { ...base, finished: true }), now)).toBe('Done · Weekdays at 9:00 AM');
  });
});
