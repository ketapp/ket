// The composer's mic, as a hook: words written into the draft where the
// cursor was, as they are said; tapped again, it closes and the last of them
// follow. See voice.ts for the listening itself.

import { type RefObject, useEffect, useRef, useState } from 'react';
import { type SharedValue, useSharedValue, withTiming } from 'react-native-reanimated';

import { Refused } from './refused';
import { type Listening, listen } from './voice';

/** How long finishing waits for the last words once the mic is closed. */
const LAST_WORDS_MS = 1500;

export interface Dictation {
  /** While listening: whether its words stay on the phone. */
  hearing: { onDevice: boolean } | null;
  /** How loud it hears, 0 to 1, for the mic's bars. */
  level: SharedValue<number>;
  /** Starts listening, or finishes. */
  toggle(): Promise<void>;
  /** Closes the mic, keeping what it heard, once its last words are in. */
  finish(): Promise<void>;
  /** Stops at once, keeping what is written: someone typed. */
  letGo(): void;
}

export function useDictation({
  local,
  draft,
  selection,
  write,
  onNote,
}: {
  /** A message to a local agent, whose words must not leave the phone. */
  local: boolean;
  /** The draft as it is this moment. */
  draft: RefObject<string>;
  /** Where the cursor is in it, which is where the words go. */
  selection: RefObject<{ start: number; end: number }>;
  write: (text: string) => void;
  /** Something to say — why it stopped, or could not start — and whether
   * Settings is where to fix it. Empty to clear the note. */
  onNote: (text: string, settings: boolean) => void;
}): Dictation {
  const [hearing, setHearing] = useState<{ onDevice: boolean } | null>(null);
  const level = useSharedValue(0);
  const listening = useRef<{ run: Listening; ended: Promise<void> } | null>(null);

  useEffect(() => {
    const live = listening;
    return () => live.current?.run.abandon();
  }, []);

  const finish = async () => {
    const current = listening.current;
    if (!current) return;
    current.run.stop();
    await Promise.race([current.ended, new Promise((resolve) => setTimeout(resolve, LAST_WORDS_MS))]);
    if (listening.current === current) {
      current.run.abandon();
      listening.current = null;
      setHearing(null);
    }
  };

  const start = async () => {
    onNote('', false);
    const text = draft.current;
    const { start: from, end: to } = selection.current;
    const before = text.slice(0, Math.min(from, text.length));
    const after = text.slice(Math.min(to, text.length));
    const lead = before && !/\s$/.test(before) ? ' ' : '';
    const tail = after && !/^\s/.test(after) ? ' ' : '';
    let finished = () => {};
    const ended = new Promise<void>((resolve) => {
      finished = resolve;
    });
    try {
      const run = await listen(local, {
        onHeard: (heard) => write(heard ? `${before}${lead}${heard}${tail}${after}` : text),
        onLevel: (loud) => level.set(withTiming(loud, { duration: 90 })),
        onEnd: (error) => {
          if (listening.current?.ended === ended) {
            listening.current = null;
            setHearing(null);
          }
          level.set(0);
          if (error) onNote(error, false);
          finished();
        },
      });
      listening.current = { run, ended };
      setHearing({ onDevice: run.onDevice });
    } catch (e) {
      onNote(e instanceof Error ? e.message : String(e), e instanceof Refused);
    }
  };

  return {
    hearing,
    level,
    toggle: () => (listening.current ? finish() : start()),
    finish,
    letGo: () => {
      const current = listening.current;
      if (!current) return;
      listening.current = null;
      current.run.abandon();
      setHearing(null);
    },
  };
}
