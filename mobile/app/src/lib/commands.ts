// The slash commands each agent has built in, for the `/` menu over a
// composer. The common ones, not every one: what is worth a tap on a phone.
//
// A command that opens a picker in the terminal — choosing a model, a
// session to resume — is sent, and then the terminal is where it is
// answered; `picker` says so.

export type Command = {
  /** With its slash: `/compact`. */
  name: string;
  /** What it does, in a few words. */
  about: string;
  /** Takes words after it: tapping fills the composer instead of sending. */
  args?: boolean;
  /** Opens a picker or panel that is answered in the terminal. */
  picker?: boolean;
};

const CLAUDE: Command[] = [
  { name: '/plan', about: 'Plan before changing anything', args: true },
  { name: '/compact', about: 'Summarise the conversation to free up context' },
  { name: '/clear', about: 'Start over with an empty conversation' },
  { name: '/context', about: 'How much of the context window is used' },
  { name: '/usage', about: 'Plan usage and when it resets' },
  { name: '/review', about: 'Review a pull request', args: true },
  { name: '/init', about: 'Write a CLAUDE.md for this project' },
  { name: '/model', about: 'Choose the model', picker: true },
  { name: '/resume', about: 'Pick up an earlier session', picker: true },
  { name: '/rewind', about: 'Go back to an earlier point', picker: true },
  { name: '/permissions', about: 'What it may do without asking', picker: true },
];

const CODEX: Command[] = [
  { name: '/compact', about: 'Summarise the conversation to free up context' },
  { name: '/new', about: 'Start a new conversation' },
  { name: '/review', about: 'Review the current changes' },
  { name: '/diff', about: 'Show the changes, including untracked files' },
  { name: '/status', about: 'Model, approvals and token use' },
  { name: '/init', about: 'Write an AGENTS.md for this project' },
  { name: '/model', about: 'Choose the model and reasoning effort', picker: true },
  { name: '/approvals', about: 'What it may do without asking', picker: true },
];

const GEMINI: Command[] = [
  { name: '/compress', about: 'Summarise the conversation to free up context' },
  { name: '/clear', about: 'Start over with an empty conversation' },
  { name: '/stats', about: 'Token use and session figures' },
  { name: '/tools', about: 'The tools it can use' },
  { name: '/memory', about: 'Show or add to its memory', args: true },
  { name: '/model', about: 'Choose the model', picker: true },
];

const OPENCODE: Command[] = [
  { name: '/compact', about: 'Summarise the conversation to free up context' },
  { name: '/new', about: 'Start a new session' },
  { name: '/undo', about: 'Undo the last change' },
  { name: '/redo', about: 'Redo what was undone' },
  { name: '/init', about: 'Write an AGENTS.md for this project' },
  { name: '/models', about: 'Choose the model', picker: true },
  { name: '/sessions', about: 'Switch to another session', picker: true },
];

// Grok Build's, as its 1.0.44 binary documents them; it has no `/init`.
const GROK: Command[] = [
  { name: '/compact', about: 'Summarise the conversation to free up context' },
  { name: '/new', about: 'Start a new session' },
  { name: '/undo', about: 'Undo the last change' },
  { name: '/usage', about: 'Plan usage and when it resets' },
  { name: '/memory', about: 'Show or add to its memory', args: true },
  { name: '/model', about: 'Choose the model', picker: true },
  { name: '/resume', about: 'Pick up an earlier session', picker: true },
  { name: '/rewind', about: 'Go back to an earlier point', picker: true },
];

/** The built-in commands of `agent` — `claude`, `codex` — or none for an
 * agent or shell this does not know. */
export function commandsFor(agent: string): Command[] {
  switch (agent.toLowerCase()) {
    case 'claude':
      return CLAUDE;
    case 'codex':
      return CODEX;
    case 'gemini':
      return GEMINI;
    case 'opencode':
      return OPENCODE;
    case 'grok':
      return GROK;
    default:
      return [];
  }
}

/** The commands a draft is asking for: while it is a slash and a partial
 * name with nothing after it, those whose name starts that way. */
export function matching(commands: Command[], draft: string): Command[] {
  if (!/^\/[\w-]*$/.test(draft)) return [];
  const typed = draft.toLowerCase();
  return commands.filter((command) => command.name.startsWith(typed));
}
