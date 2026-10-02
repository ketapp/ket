// The `/` menu: an agent's slash commands, over the composer while a draft
// is a slash and the start of a name. See lib/commands.ts for the lists.

import { ScrollView, StyleSheet } from 'react-native';

import type { Command } from '../lib/commands';
import { t } from '../lib/theme';
import { Row, Tag } from './ui';

export function CommandMenu({ commands, onPick }: { commands: Command[]; onPick: (command: Command) => void }) {
  return (
    <ScrollView style={styles.menu} contentContainerStyle={styles.inner} keyboardShouldPersistTaps="handled">
      {commands.map((command) => (
        <Row
          key={command.name}
          title={command.name}
          detail={command.about}
          meta={command.picker ? <Tag>in terminal</Tag> : undefined}
          onPress={() => onPick(command)}
        />
      ))}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  menu: { flexGrow: 0, maxHeight: 248, borderTopWidth: 1, borderTopColor: t.rule, backgroundColor: t.backdrop },
  inner: { padding: 6, gap: 2 },
});
