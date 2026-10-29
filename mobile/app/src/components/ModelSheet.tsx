// The session sheet: the agent's mode, model and reasoning effort in one
// place, opened from the chat bar's pill (1.26). Each pick applies at once,
// by the agent's own command on the desktop; while one is going through it
// is the one marked, the rest wait, and the foot says so — then how it went.

import type { AgentChoice, AgentModes, AgentSettings } from '@ket/remote';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { font, t } from '../lib/theme';
import { Sheet, SheetItem, SheetList, SheetNote } from './Sheet';

/** A row of choices, the current one raised, as the desktop's segmented
 * controls are. `pending` is the label of one going through, raised in the
 * current one's place from the tap. */
function Segments({
  choices,
  current,
  pending,
  disabled,
  onPick,
}: {
  choices: AgentChoice[];
  current: string;
  pending: string;
  disabled: boolean;
  onPick: (choice: AgentChoice) => void;
}) {
  const moving = choices.some((choice) => choice.label === pending);
  return (
    <View style={styles.segments} accessibilityRole="radiogroup">
      {choices.map((choice) => {
        const on = choice.id === current && !moving;
        const going = moving && pending === choice.label;
        return (
          <Pressable
            key={choice.id}
            accessibilityRole="radio"
            accessibilityState={{ checked: on || going, disabled }}
            accessibilityLabel={choice.label}
            disabled={disabled || on}
            onPress={() => onPick(choice)}
            style={({ pressed }) => [
              styles.segment,
              (on || going) && styles.segmentOn,
              pressed && { backgroundColor: t.selection },
            ]}
          >
            <Text
              style={[styles.segmentText, (on || going) && styles.segmentTextOn, disabled && !on && !going && { color: t.faint }]}
              numberOfLines={1}
              adjustsFontSizeToFit
              minimumFontScale={0.8}
            >
              {going ? `${choice.label}…` : choice.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

export function ModelSheet({
  open,
  onClose,
  agent,
  modes,
  settings,
  pending,
  said,
  busy,
  onMode,
  onModel,
  onEffort,
}: {
  open: boolean;
  onClose: () => void;
  /** The agent's name, as a person reads it: Claude, Codex. */
  agent: string;
  modes: AgentModes | undefined;
  settings: AgentSettings;
  /** The label of a pick going through, or empty. */
  pending: string;
  /** How the last pick went, until the next one. */
  said: { text: string; failed: boolean } | null;
  /** The agent is mid-turn: nothing can be changed until it is done. */
  busy: boolean;
  onMode: (choice: AgentChoice) => void;
  onModel: (choice: AgentChoice) => void;
  onEffort: (choice: AgentChoice) => void;
}) {
  const held = busy || pending !== '';
  const moving = settings.models.some((model) => model.label === pending);
  return (
    <Sheet open={open} onClose={onClose} title={agent} detail="This session" label={`${agent} settings`}>
      {modes && modes.choices.length > 0 ? (
        <View style={styles.group}>
          <Text style={styles.label}>Mode</Text>
          <Segments
            choices={modes.choices.map((mode) => ({ id: mode.id, label: mode.label }) as AgentChoice)}
            current={modes.current}
            pending={pending}
            disabled={held}
            onPick={onMode}
          />
        </View>
      ) : null}
      <View style={styles.group}>
        <Text style={styles.label}>Model</Text>
        <SheetList>
          {settings.models.map((model) => {
            const going = pending !== '' && pending === model.label;
            return (
              <SheetItem
                key={model.id}
                role="radio"
                label={going ? `${model.label}…` : model.label}
                on={moving ? going : model.id === settings.model}
                held={held}
                onPress={() => onModel(model)}
              />
            );
          })}
        </SheetList>
      </View>
      <View style={styles.group}>
        <Text style={styles.label}>Effort</Text>
        <Segments choices={settings.efforts} current={settings.effort} pending={pending} disabled={held} onPick={onEffort} />
        <View accessibilityLiveRegion="polite">
          <SheetNote>
            {pending ? (
              `Switching to ${pending}…`
            ) : said ? (
              <Text style={{ color: said.failed ? t.failed : t.text }}>{said.text}</Text>
            ) : busy ? (
              `${agent} is working. These can change once it's done.`
            ) : settings.savesDefault ? (
              `${agent} also keeps your pick as its default for new sessions.`
            ) : (
              'For this session only.'
            )}
          </SheetNote>
        </View>
      </View>
    </Sheet>
  );
}

const styles = StyleSheet.create({
  group: { gap: 8 },
  label: { fontFamily: font.proseMedium, fontSize: 13, color: t.dim },
  segments: { flexDirection: 'row', gap: 6, padding: 4, borderRadius: 12, backgroundColor: t.sunken },
  segment: { flex: 1, height: 38, borderRadius: 9, alignItems: 'center', justifyContent: 'center', paddingHorizontal: 4 },
  segmentOn: { backgroundColor: t.selection },
  segmentText: { fontFamily: font.prose, fontSize: 13.5, color: t.dim },
  segmentTextOn: { fontFamily: font.proseMedium, color: t.text },
});
