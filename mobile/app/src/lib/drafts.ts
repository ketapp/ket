// What was typed in each chat's composer and not sent yet, kept on this
// phone: a crash, an update or leaving the chat leaves it in the field for
// coming back to. Sending clears it.
//
// By chat, as the chat screen keys what it keeps (host, terminal, and an
// agent view session), with the worktree it was typed in: the Mac numbers its
// terminals from 0 each time ket opens, so after a restart the same number
// can be another worktree's tab, and a draft is only put back in its own.
//
// One small file in the app's own storage, read once and written a moment
// after typing stops, and at once when the app goes to the background.

import { File, Paths } from 'expo-file-system';
import { AppState } from 'react-native';

interface Draft {
  text: string;
  /** The worktree's id, from the desktop's snapshot. */
  where: string;
  /** When it was last typed in, for letting go of the oldest. */
  at: number;
}

/** How many chats keep a draft: past this, the oldest goes. */
const KEEP = 50;
/** How long after the last change the file is written. */
const WRITE_AFTER_MS = 400;
const NAME = 'drafts.json';

let drafts: Record<string, Draft> | undefined;
let timer: ReturnType<typeof setTimeout> | undefined;
let listening = false;

function file(): File {
  return new File(Paths.document, NAME);
}

function load(): Record<string, Draft> {
  if (drafts) return drafts;
  try {
    const target = file();
    const text = target.exists ? target.textSync() : '';
    const parsed: unknown = text ? JSON.parse(text) : {};
    drafts = parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, Draft>) : {};
  } catch {
    drafts = {};
  }
  return drafts;
}

function flush(): void {
  if (timer !== undefined) clearTimeout(timer);
  timer = undefined;
  if (!drafts) return;
  try {
    const target = file();
    if (!target.exists) target.create();
    target.writeSync(JSON.stringify(drafts));
  } catch {
    // Kept in memory still: the next change tries again.
  }
}

/** The draft kept for `key`, if it was typed in worktree `where`. */
export function draftFor(key: string, where: string): string {
  const kept = load()[key];
  return kept && kept.where === where ? kept.text : '';
}

/** Keeps `text` as `key`'s draft, in worktree `where`; empty lets it go. */
export function keepDraft(key: string, where: string, text: string): void {
  const all = load();
  const before = all[key];
  if (!text.trim()) {
    if (!before) return;
    delete all[key];
  } else {
    if (before?.text === text && before.where === where) return;
    all[key] = { text, where, at: Date.now() };
    const keys = Object.keys(all);
    if (keys.length > KEEP) {
      keys.sort((a, b) => all[a].at - all[b].at);
      for (const old of keys.slice(0, keys.length - KEEP)) delete all[old];
    }
  }
  if (!listening) {
    listening = true;
    AppState.addEventListener('change', (state) => {
      if (state !== 'active') flush();
    });
  }
  if (timer !== undefined) clearTimeout(timer);
  timer = setTimeout(flush, WRITE_AFTER_MS);
}
