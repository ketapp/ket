// The `/` menu: an agent's slash commands, in a float over the chat while
// a draft is a slash and the start of a name. See lib/commands.ts for the
// lists.

import { ScrollView, StyleSheet } from 'react-native';

import type { Command } from '../lib/commands';
import { Stagger } from './Composer';
import { Row, Tag } from './ui';

export function CommandMenu({ commands, onPick }: { commands: Command[]; onPick: (command: Command) => void }) {
  return (
    <ScrollView style={styles.menu} contentContainerStyle={styles.inner} keyboardShouldPersistTaps="handled">
      {commands.map((command, index) => (
        <Stagger key={command.name} index={index}>
          <Row
            title={command.name}
            detail={command.about}
            meta={command.picker ? <Tag>in terminal</Tag> : undefined}
            onPress={() => onPick(command)}
          />
        </Stagger>
      ))}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  // In a float, which draws the card — see components/Composer.tsx.
  menu: { flexGrow: 0, maxHeight: 248 },
  inner: { padding: 6, gap: 2 },
});
