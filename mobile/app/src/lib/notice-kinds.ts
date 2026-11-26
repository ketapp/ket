// The kinds of notice the phone can show, as Settings lists them, each with
// its own switch: apart from lib/notices, which sends them, so a screen that
// only names them needs nothing more.

import type { Kind } from './notices';

/** The notices, each with its switch in Settings, in the order they are asked for. */
export const NOTICES: { kind: Kind; name: string; detail: string }[] = [
  { kind: 'waiting', name: 'An agent is waiting on you', detail: 'It stopped on a permission prompt or a question.' },
  { kind: 'finished', name: 'A task finishes', detail: 'The agent is done, or failed, with its last words.' },
  { kind: 'checks', name: 'Checks fail', detail: 'A check on a worktree\'s pull request has failed.' },
  { kind: 'limits', name: 'A plan runs out', detail: 'An agent reached its usage limit mid-task, or the limit has reset.' },
];
