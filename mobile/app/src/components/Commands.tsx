// The `/` menu: an agent's slash commands, while a draft is a slash and the
// start of a name — in the app's one menu, in place over the chat, or as a
// list in the terminal's panel. See lib/commands.ts for the lists.

import type { Command } from '../lib/commands';
import { type MenuItem, MenuList } from './FloatMenu';
import { Tag } from './ui';

/** One of the `/` menu's rows: a command, what it does, and whether it opens
 * a picker in the terminal. Its press is the caller's to add. */
export function commandRow(command: Command): Omit<MenuItem, 'onPress'> {
  return {
    key: command.name,
    label: command.name,
    detail: command.about,
    icon: command.picker ? <Tag>in terminal</Tag> : undefined,
  };
}

/** The commands as a list of the menu's rows, where a panel holds them. */
export function CommandMenu({ commands, onPick }: { commands: Command[]; onPick: (command: Command) => void }) {
  return (
    <MenuList
      sections={[{ items: commands.map((command) => ({ ...commandRow(command), onPress: () => onPick(command) })) }]}
      onChoose={(item) => item.onPress()}
      maxHeight={248}
    />
  );
}
