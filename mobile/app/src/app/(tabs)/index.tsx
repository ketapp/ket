// Worktrees. On a phone, the list itself. Beside the sidebar on a wide
// screen, where the list already is, the space a worktree opens into, until
// one is picked.

import { StyleSheet, View } from 'react-native';

import { WorktreeList } from '../../components/WorktreeList';
import { BrandMark } from '../../components/icons';
import { Prose, Screen } from '../../components/ui';
import { useSplit } from '../../lib/layout';
import { t } from '../../lib/theme';

export default function Worktrees() {
  const split = useSplit();
  if (!split) return <WorktreeList />;
  return (
    <Screen>
      <View style={styles.empty}>
        <BrandMark size={22} />
        <Prose color={t.dim}>Pick a worktree to follow its agent.</Prose>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  empty: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: 14 },
});
