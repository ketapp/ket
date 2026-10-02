// A desktop, by id — where pairing lands and old links point. The Worktrees
// screen shows one desktop at a time, so this makes that desktop the one it
// shows and goes there.

import { Redirect, useLocalSearchParams } from 'expo-router';
import { useEffect, useState } from 'react';

import { Screen } from '../../components/ui';
import { saveDesktop } from '../../lib/store';

export default function Host() {
  const { host: id } = useLocalSearchParams<{ host: string }>();
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    void saveDesktop(id).then(() => setSaved(true));
  }, [id]);
  return saved ? <Redirect href="/" /> : <Screen>{null}</Screen>;
}
