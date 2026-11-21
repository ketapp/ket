// The chat's bar — the session's name, the agent's mode, model and effort,
// and where to go from here — and the note hung under it, over the
// conversation.

import type { ReactNode } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';
import Animated, { FadeIn, FadeOut } from 'react-native-reanimated';

import { Aside, Changes, Check, Clipboard as ClipboardGlyph, Exclaim, TerminalWindow } from './icons';
import { SessionPill, type SessionPicks } from './SessionPicks';
import { Bar, Button, IconCluster } from './ui';
import { font, size, t, tint } from '../lib/theme';

export function ChatBar({
  title,
  picks,
  connected,
  busy,
  onWorktree,
  onTerminal,
}: {
  title: string;
  picks: SessionPicks;
  /** The desktop is there to take a pick. */
  connected: boolean;
  /** The agent is mid-turn: its mode waits until it is done. */
  busy: boolean;
  /** The worktree's screen, when the session is in one. */
  onWorktree?: () => void;
  onTerminal: () => void;
}) {
  return (
    <Bar
      back
      title={title}
      right={
        <View style={styles.actions}>
          <SessionPill picks={picks} connected={connected} busy={busy} />
          {/* Where you can go from here, in one frame; the pill stands
              alone because it changes the session rather than leaving it. */}
          <IconCluster
            actions={[
              ...(onWorktree ? [{ label: 'The worktree', onPress: onWorktree, icon: <Changes size={18} color={t.text} /> }] : []),
              { label: 'Open the terminal', onPress: onTerminal, icon: <TerminalWindow size={18} color={t.text} /> },
            ]}
          />
        </View>
      }
    />
  );
}

/** What a note is: the capsule's mark and its colour say which. */
export type Tone = 'done' | 'copied' | 'warn' | 'info';

const TONE: Record<Tone, { color: () => string; icon: (color: string) => ReactNode }> = {
  done: { color: () => t.running, icon: (color) => <Check size={14} color={color} weight={2.2} /> },
  copied: { color: () => t.accent, icon: (color) => <ClipboardGlyph size={14} color={color} weight={2} /> },
  warn: { color: () => t.failed, icon: (color) => <Exclaim size={14} color={color} weight={2.4} /> },
  info: { color: () => t.dim, icon: (color) => <Aside size={14} color={color} weight={2.4} /> },
};

/** The note: a capsule hung under the bar, over the conversation, so nothing
 * under it moves as it comes and goes. A tap puts it away; holding its words
 * copies them. `toSettings` is a permission that is off: Settings, under it. */
export function ChatNote({
  text,
  tone,
  toSettings,
  onDismiss,
  onCopy,
}: {
  text: string;
  tone: Tone;
  toSettings: boolean;
  onDismiss: () => void;
  onCopy: (text: string) => void;
}) {
  return (
    <View pointerEvents="box-none" style={styles.anchor}>
      {text ? (
        <Animated.View key={text} entering={FadeIn.duration(160)} exiting={FadeOut.duration(160)} pointerEvents="box-none" style={styles.hang}>
          <Pressable
            accessibilityLiveRegion="polite"
            accessibilityHint="Tap to put away, hold to copy"
            onPress={onDismiss}
            delayLongPress={350}
            onLongPress={() => onCopy(text)}
            style={({ pressed }) => [styles.capsule, pressed && { opacity: 0.85 }]}
          >
            <View style={[styles.mark, { backgroundColor: tint(TONE[tone].color(), 0.18) }]}>{TONE[tone].icon(TONE[tone].color())}</View>
            <View style={styles.words}>
              <Text style={styles.text}>{text}</Text>
              {toSettings ? <Button title="Open Settings" onPress={() => void Linking.openSettings()} style={styles.settings} /> : null}
            </View>
          </Pressable>
        </Animated.View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  actions: { flexDirection: 'row', gap: 6, alignItems: 'center' },
  // The note's capsule: hung from a line of no height under the bar, drawn
  // over the conversation rather than taking room from it.
  anchor: { height: 0, zIndex: 3, elevation: 3 },
  hang: { position: 'absolute', top: 8, left: size.gutter + 14, right: size.gutter + 14, alignItems: 'center' },
  capsule: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 9,
    maxWidth: '100%',
    paddingVertical: 7,
    paddingLeft: 7,
    paddingRight: 16,
    borderRadius: 20,
    borderWidth: 1,
    borderColor: t.border,
    backgroundColor: t.elevated,
    shadowColor: '#000',
    shadowOpacity: 0.55,
    shadowRadius: 14,
    shadowOffset: { width: 0, height: 8 },
    elevation: 8,
  },
  mark: { width: 24, height: 24, borderRadius: 12, alignItems: 'center', justifyContent: 'center', alignSelf: 'flex-start' },
  words: { flexShrink: 1, gap: 8, paddingVertical: 2.5 },
  text: { fontFamily: font.proseMedium, fontSize: 13.5, lineHeight: 19, color: t.text },
  settings: { height: 34, paddingHorizontal: 14, alignSelf: 'flex-start' },
});
