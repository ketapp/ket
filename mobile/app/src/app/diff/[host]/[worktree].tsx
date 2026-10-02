'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// One file's changes in a worktree, as a merge would bring them: the lines
// added and removed, each numbered, wrapped to the phone's width rather than
// scrolled sideways. Read-only — for looking before merging.

import { useLocalSearchParams } from 'expo-router';
import { useEffect, useState } from 'react';
import { FlatList, StyleSheet, Text, View } from 'react-native';

import { Bar, Prose, Screen } from '../../../components/ui';
import { useHost } from '../../../lib/hosts';
import { font, size, t, tint } from '../../../lib/theme';

/** Most lines drawn; a longer diff is cut here with a note. */
const MOST_LINES = 3000;

type Line = { kind: 'add' | 'remove' | 'same' | 'hunk'; text: string; number: string };

export default function DiffScreen() {
  const { host: hostId, worktree: raw, path } = useLocalSearchParams<{ host: string; worktree: string; path: string }>();
  const id = decodeURIComponent(raw);
  const conn = useHost(hostId);
  const [lines, setLines] = useState<Line[] | null>(null);
  const [cut, setCut] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!conn || !path) return;
    let stopped = false;
    conn
      .diff(id, path)
      .then((reply) => {
        if (stopped) return;
        if (reply.error) {
          setError(reply.error);
          return;
        }
        const parsed = parse(reply.text);
        setLines(parsed.slice(0, MOST_LINES));
        setCut(reply.truncated || parsed.length > MOST_LINES);
      })
      .catch((e: unknown) => {
        if (!stopped) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stopped = true;
    };
  }, [conn, id, path]);

  const name = path?.split('/').pop() ?? 'Diff';
  const folder = path?.includes('/') ? path.slice(0, path.lastIndexOf('/')) : undefined;

  return (
    <Screen>
      <Bar back title={name} detail={folder} />
      {error ? (
        <Prose style={styles.message}>{error.charAt(0).toUpperCase() + error.slice(1)}</Prose>
      ) : lines === null ? (
        <Prose style={styles.message} color={t.dim}>
          Loading…
        </Prose>
      ) : lines.length === 0 ? (
        <Prose style={styles.message} color={t.dim}>
          No line changes — a binary file, or only its mode changed.
        </Prose>
      ) : (
        <FlatList
          data={lines}
          keyExtractor={(_, index) => String(index)}
          renderItem={({ item }) => <LineView line={item} />}
          initialNumToRender={60}
          windowSize={11}
          contentContainerStyle={styles.page}
          ListFooterComponent={
            cut ? (
              <Prose style={styles.message} color={t.dim}>
                The rest is too long to show here; see it on your Mac.
              </Prose>
            ) : null
          }
        />
      )}
    </Screen>
  );
}

function LineView({ line }: { line: Line }) {
  if (line.kind === 'hunk') {
    return (
      <View style={styles.hunk}>
        <Text style={styles.hunkText} numberOfLines={1}>
          {line.text}
        </Text>
      </View>
    );
  }
  const color = line.kind === 'add' ? t.added : line.kind === 'remove' ? t.removed : undefined;
  return (
    <View style={[styles.line, color ? { backgroundColor: tint(color, 0.1) } : null]}>
      <Text style={[styles.number, color ? { color } : null]}>{line.number}</Text>
      <Text style={[styles.sign, color ? { color } : null]}>
        {line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}
      </Text>
      <Text style={styles.text}>{line.text || ' '}</Text>
    </View>
  );
}

/** A unified diff as lines to draw: the file's headers dropped, each hunk
 * marked where it starts, each line numbered — in the new file, or the old
 * one for a line taken out. */
function parse(diff: string): Line[] {
  const lines: Line[] = [];
  let old = 0;
  let now = 0;
  let inHunk = false;
  for (const raw of diff.split('\n')) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@ ?(.*)$/.exec(raw);
    if (hunk) {
      old = Number(hunk[1]);
      now = Number(hunk[2]);
      inHunk = true;
      lines.push({ kind: 'hunk', text: hunk[3] ? `Line ${now} · ${hunk[3]}` : `Line ${now}`, number: '' });
      continue;
    }
    if (!inHunk || raw.startsWith('\\')) continue;
    if (raw.startsWith('diff --git')) {
      inHunk = false;
      continue;
    }
    const text = raw.slice(1).replace(/\t/g, '  ');
    if (raw.startsWith('+')) lines.push({ kind: 'add', text, number: String(now++) });
    else if (raw.startsWith('-')) lines.push({ kind: 'remove', text, number: String(old++) });
    else if (raw.startsWith(' ')) {
      lines.push({ kind: 'same', text, number: String(now++) });
      old++;
    }
  }
  return lines;
}

const styles = StyleSheet.create({
  page: { paddingBottom: 24 },
  message: { paddingHorizontal: size.gutter + 4, paddingVertical: 16 },
  hunk: {
    marginTop: 10,
    paddingHorizontal: size.gutter + 4,
    paddingVertical: 6,
    backgroundColor: t.sunken,
  },
  hunkText: { fontFamily: font.chrome, fontSize: size.caption, color: t.dim },
  line: { flexDirection: 'row', paddingHorizontal: 6 },
  number: {
    width: 36,
    textAlign: 'right',
    fontFamily: font.code,
    fontSize: 11,
    lineHeight: 18,
    color: t.faint,
  },
  sign: { width: 16, textAlign: 'center', fontFamily: font.code, fontSize: 12, lineHeight: 18, color: t.faint },
  text: { flex: 1, minWidth: 0, fontFamily: font.code, fontSize: 12, lineHeight: 18, color: t.text },
});
