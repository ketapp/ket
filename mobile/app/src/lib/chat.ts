// A conversation's parts that are only data: messages sent from here and
// how the conversation reads them back, and the turns as the chat groups
// them. What the chat (app/session/[host]/[terminal].tsx) reasons with, kept
// apart from what it draws.

import { type Turn, Turn_Kind } from '@ket/remote';

import { type Attachment, splitFiles } from './attach';

/** A message sent from here that the conversation has not shown yet. */
export type Pending = {
  key: number;
  text: string;
  at: number;
  /** Sent while the agent was working, so it waits in the agent's queue,
   * after the step in progress. Sent while it was idle, it is what starts
   * the next step, and belongs before the step's indicator. */
  behind: boolean;
  /** The photos and files sent with it. */
  files: Attachment[];
};

/** A message of yours as the conversation reads it back: its words, without
 * the paths the desktop listed under them, and how many images it carried. */
export type Said = { words: string; images: number };

/** Appends `more`, running an agent's reply on when it arrived in two reads. */
export function join(before: Turn[], more: Turn[]): Turn[] {
  const last = before[before.length - 1];
  const [first, ...rest] = more;
  if (last && first && last.kind === Turn_Kind.ASSISTANT && first.kind === Turn_Kind.ASSISTANT) {
    return [...before.slice(0, -1), { ...last, text: `${last.text}\n\n${first.text}` }, ...rest];
  }
  return [...before, ...more];
}

/** A message of yours as the conversation reads it back. */
export function saidIn(turn: Turn): Said {
  return { words: splitFiles(turn.text).words, images: turn.images ?? 0 };
}

/** Whether `said`, read back, is pending message `sent`: the same words —
 * see [`carries`] — or, for photos sent alone, images and no words. */
export function fits(said: Said, sent: Pending): boolean {
  if (!sent.text) return said.words === '' && said.images > 0;
  return carries(said.words, sent.text);
}

/** Whether a message the transcript or the agent's queue shows is one sent
 * from here: the same start, since the desktop clips long turns, or the same
 * end, since whatever the terminal's prompt already held when the phone
 * typed — a stray key at the desktop — arrives in front of it. */
export function carries(said: string, sent: string): boolean {
  const flat = (text: string) => text.replace(/\s+/g, ' ').trim();
  const whole = flat(said);
  const mine = flat(sent);
  return whole.slice(0, 200) === mine.slice(0, 200) || whole.endsWith(mine);
}

/** The conversation as it is drawn: a run of tool lines — one alone too —
 * as one group, shut, every other turn on its own. Each is keyed by the turn
 * it starts at, so a group the agent is still adding to stays as open as it
 * was, and its first line does not open out and shut again as the second
 * arrives. */
export function grouped(turns: Turn[]): { at: number; tools?: Turn[] }[] {
  const items: { at: number; tools?: Turn[] }[] = [];
  let at = 0;
  while (at < turns.length) {
    let end = at;
    while (end < turns.length && turns[end].kind === Turn_Kind.TOOL) end += 1;
    if (end > at) {
      items.push({ at, tools: turns.slice(at, end) });
      at = end;
    } else {
      items.push({ at });
      at += 1;
    }
  }
  return items;
}
