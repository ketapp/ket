// When a chat began, centred over its first message as Messages puts it:
// "Today 2:14 PM", "Yesterday 9:03 AM", "Tue 24 Nov 4:40 PM". The day in a
// heavier weight, the time beside it in grey. From the desktop's read of the
// session (protocol 1.41); a desktop that can't tell sends zero and nothing
// is drawn.

import { StyleSheet, Text } from 'react-native';

import { Prose } from './ui';
import { font, t } from '../lib/theme';

const DAY_MS = 24 * 60 * 60 * 1000;

/** "Today", "Yesterday", or the date: with the year once it isn't this one. */
export function dayOf(at: Date, now: Date): string {
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((midnight(now) - midnight(at)) / DAY_MS);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  return at.toLocaleDateString(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    ...(at.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }),
  });
}

/** `now` is the chat's own clock, which every poll moves on, so a chat left
 * open past midnight stops saying "Today". */
export function ChatStart({ at, now }: { at: number; now: number }) {
  const when = new Date(at);
  const time = when.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  const day = dayOf(when, new Date(now));
  return (
    <Prose style={styles.line}>
      <Text style={styles.day}>{day}</Text>
      {` ${time}`}
    </Prose>
  );
}

const styles = StyleSheet.create({
  line: { textAlign: 'center', fontSize: 11.5, color: t.dim, paddingTop: 4, paddingBottom: 2 },
  day: { fontFamily: font.proseSemibold, color: t.text },
});
