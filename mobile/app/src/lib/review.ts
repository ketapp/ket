// A worktree's review on the phone: comments on lines of its diff, kept
// until Send to agent hands them all to its agent as one message, the way
// the desktop's Review section does (desktop 1.14). ket on the desktop
// writes the message — see HostConnection.sendReview.
//
// In memory only, as the desktop's review is: a draft of one message, not a
// record, and gone if iOS ends the app.

import { useSyncExternalStore } from 'react';

import type { HostConnection } from './connection';

/** Where a comment's card sits in the diff: under the last line picked, for
 * as long as the diff still shows that line — numbered the same in the same
 * file's numbering, and reading the same. */
export interface Anchor {
  side: 'new' | 'old';
  number: number;
  text: string;
}

/** One comment waiting to be sent. */
export interface ReviewComment {
  id: number;
  /** The file, as Changes lists it. */
  path: string;
  /** Which lines, as the desktop says them: `lines 12–18`. */
  lines: string;
  /** The lines themselves, each after its `+`, `-` or space. */
  text: string;
  /** What should change. */
  comment: string;
  anchor: Anchor;
}

/** The most one review sends, as the desktop takes it: comments, and bytes
 * of text in them all — see `review_fits` in ket-core's phone requests. A
 * review never grows past either, so it can always be sent. */
const MOST_COMMENTS = 50;
const MOST_BYTES = 64 * 1024;

/** What a comment sends: its file, lines, the lines' text and its words. */
type Sent = Pick<ReviewComment, 'path' | 'lines' | 'text' | 'comment'>;

const reviews = new Map<string, ReviewComment[]>();
const listeners = new Set<() => void>();
const NONE: ReviewComment[] = [];
let nextId = 1;

function keyOf(host: string, worktree: string): string {
  return `${host}/${worktree}`;
}

/** Each change makes a new list, so a screen holding the old one redraws. */
function put(host: string, worktree: string, comments: ReviewComment[]): void {
  const key = keyOf(host, worktree);
  if (comments.length > 0) reviews.set(key, comments);
  else reviews.delete(key);
  listeners.forEach((listener) => listener());
}

export const review = {
  list(host: string, worktree: string): ReviewComment[] {
    return reviews.get(keyOf(host, worktree)) ?? NONE;
  },

  add(host: string, worktree: string, comment: Omit<ReviewComment, 'id'>): void {
    put(host, worktree, [...review.list(host, worktree), { ...comment, id: nextId++ }]);
  },

  edit(host: string, worktree: string, id: number, comment: string): void {
    put(
      host,
      worktree,
      review.list(host, worktree).map((c) => (c.id === id ? { ...c, comment } : c)),
    );
  },

  /** Takes out the comments named, or every one. */
  remove(host: string, worktree: string, ids?: number[]): void {
    put(host, worktree, ids ? review.list(host, worktree).filter((c) => !ids.includes(c.id)) : []);
  },

  /** Takes out the comments that were sent, as they were sent. One edited
   * since is a new one and stays: its new words have not gone. */
  sent(host: string, worktree: string, sent: ReviewComment[]): void {
    put(host, worktree, review.list(host, worktree).filter((c) => !sent.includes(c)));
  },
};

/** Why a review is full: no room for another comment — or, for `editing`,
 * for its new words — or `null` while there is. */
export function reviewFull(host: string, worktree: string, next: Sent | null, editing?: number): string | null {
  const others = review.list(host, worktree).filter((c) => c.id !== editing);
  if (editing === undefined && others.length >= MOST_COMMENTS) {
    return `The review has ${MOST_COMMENTS} comments, the most one sends. Send it to the agent first.`;
  }
  if (!next) return null;
  const mine = bytesOf(next);
  if (mine > MOST_BYTES) return 'That’s more than one review can send. Pick fewer lines, or write less.';
  if (others.reduce((sum, c) => sum + bytesOf(c), mine) > MOST_BYTES) {
    return editing === undefined
      ? 'The review is as long as one can be. Send it to the agent first.'
      : 'That makes the review longer than one can be. Write less.';
  }
  return null;
}

/** A comment's size as the desktop counts it: bytes of UTF-8. */
function bytesOf(comment: Sent): number {
  let bytes = 0;
  for (const text of [comment.path, comment.lines, comment.text, comment.comment]) {
    for (const char of text) {
      const code = char.codePointAt(0) ?? 0;
      bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    }
  }
  return bytes;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** A worktree's review, redrawn as it changes. */
export function useReview(host: string, worktree: string): ReviewComment[] {
  return useSyncExternalStore(subscribe, () => review.list(host, worktree));
}

/** Which lines, as the desktop's diff says them: `line 12`, `lines 12–18`. */
export function linesLabel(first: number, last: number): string {
  return first === last ? `line ${first}` : `lines ${first}–${last}`;
}

/** A card's heading: `Lines 12–18`. */
export function linesHeading(lines: string): string {
  return lines ? lines.charAt(0).toUpperCase() + lines.slice(1) : 'The hunk';
}

/** The lines picked, as a review quotes them: each after its sign. */
export function pickText(lines: { kind: 'add' | 'remove' | 'same'; text: string }[]): string {
  return lines.map((line) => (line.kind === 'add' ? '+' : line.kind === 'remove' ? '-' : ' ') + line.text).join('\n');
}

/** `1 comment`, `3 comments`. */
export function commentsSaid(count: number): string {
  return count === 1 ? '1 comment' : `${count} comments`;
}

/** Sends a worktree's review to its agent and, once the agent has it, takes
 * those comments out of it — but not one edited meanwhile, whose new words
 * are still to send. What to say either way: the desktop's reason when it
 * would not send, and the comments are kept to send again. */
export async function sendReview(
  conn: HostConnection,
  host: string,
  worktree: string,
): Promise<{ text: string; failed: boolean }> {
  const comments = review.list(host, worktree);
  if (comments.length === 0) return { text: 'Nothing in the review yet.', failed: false };
  const why = await conn
    .sendReview(
      worktree,
      comments.map(({ path, lines, text, comment }) => ({ path, lines, text, comment })),
    )
    .catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
  if (why) return { text: why.charAt(0).toUpperCase() + why.slice(1), failed: true };
  review.sent(host, worktree, comments);
  const edited = review.list(host, worktree).filter((c) => comments.some((sent) => sent.id === c.id)).length;
  const sent = `Sent ${commentsSaid(comments.length)} to the agent.`;
  if (edited === 0) return { text: sent, failed: false };
  const still = edited === 1 ? 'One was edited while it went, and is still here' : `${edited} were edited while they went, and are still here`;
  return { text: `${sent} ${still} to send again.`, failed: false };
}
