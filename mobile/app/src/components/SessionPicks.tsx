// The agent's mode, model and reasoning effort, from the chat's bar: the pill
// that says them, the menu or sheet it opens, and each pick standing in for
// what the desktop's snapshot says until the snapshot reports it.

import type { AgentModes, AgentSettings } from '@ket/remote';
import { useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, View, type ViewInstance } from 'react-native';

import type { Tone } from './ChatBar';
import { FloatMenu, type Place, placeOf } from './FloatMenu';
import { Bolt, Check, Pause, Pencil, Shield, Sliders } from './icons';
import { ModelSheet } from './ModelSheet';
import type { HostConnection } from '../lib/connection';
import { font, t } from '../lib/theme';

/** Picks the desktop has made and its snapshot hasn't reported yet: each
 * the value picked, and what the snapshot said when it was picked. */
type Landed = Partial<Record<'mode' | 'model' | 'effort', { to: string; from: string }>>;

/** Has the desktop do something that needs the terminal's control: the
 * chat's `controlled`. `false` when it didn't; why not goes to `say`. */
type Controlled = (
  act: () => Promise<boolean>,
  failed: (why: string) => string,
  say?: (text: string) => void,
) => Promise<boolean>;

export type SessionPicks = ReturnType<typeof useSessionPicks>;

export function useSessionPicks({
  conn,
  terminalId,
  reported,
  told,
  busy,
  controlled,
  setNote,
}: {
  conn: HostConnection | null;
  terminalId: bigint;
  /** The agent's modes, as the desktop can tell them (1.24): Claude's
   * permission mode, OpenCode's agent, Codex's collaboration mode. Picking
   * one has the desktop press the agent's own key until it reports it — a
   * message asking it to implement cannot change its mode. */
  reported: AgentModes | undefined;
  /** Its model and reasoning effort (1.26), Claude's and Codex's: read off
   * its transcript, changed by its own commands on the desktop. */
  told: AgentSettings | undefined;
  /** Whether the agent is mid-turn, as it is the moment a pick is made. */
  busy: { readonly current: boolean };
  controlled: Controlled;
  /** The chat's note, over the conversation. */
  setNote: (text: string, tone?: Tone) => void;
}) {
  // The mode's menu, where the pill is, and the mode being switched to
  // while the desktop presses for it.
  const [modeMenu, setModeMenu] = useState<Place | null>(null);
  const [switching, setSwitching] = useState('');
  // How the last pick went, said in the sheet that made it: the chat's own
  // note is under the sheet while it is up.
  const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
  // Picks the desktop has made, each with what the snapshot said when it was
  // picked: shown in its place until the snapshot moves off that — to the
  // pick, a second or so later — rather than flicking back in between.
  const [landed, setLanded] = useState<Landed | null>(null);
  // The session sheet — mode, model and effort — for an agent with models (1.26).
  const [sheet, setSheet] = useState(false);

  // Each with a pick just made standing in until the snapshot moves. Retired
  // the moment it does — to the pick, or past it from the Mac — so it never
  // stands in again over a later change.
  const reporting: Record<keyof Landed, string | undefined> = {
    mode: reported?.current,
    model: told?.model,
    effort: told?.effort,
  };
  const picks = Object.entries(landed ?? {}) as [keyof Landed, { to: string; from: string }][];
  const standing = picks.filter(([what, pick]) => pick.from === reporting[what]);
  if (landed && standing.length < picks.length) {
    setLanded(standing.length > 0 ? Object.fromEntries(standing) : null);
  }
  const shown = (what: keyof Landed, value: string) => {
    const pick = landed?.[what];
    return pick && pick.from === value ? pick.to : value;
  };
  const modes = reported && landed?.mode ? { ...reported, current: shown('mode', reported.current) } : reported;
  const settings =
    told && (landed?.model || landed?.effort)
      ? { ...told, model: shown('model', told.model), effort: shown('effort', told.effort) }
      : told;
  // A desktop that never reports it — a session with no transcript yet —
  // has what it says shown again soon after.
  useEffect(() => {
    if (!landed) return;
    const timer = setTimeout(() => setLanded(null), 5000);
    return () => clearTimeout(timer);
  }, [landed]);
  const currentMode = modes?.choices.find((choice) => choice.id === modes.current);
  const modelLabel = settings?.models.find((choice) => choice.id === settings.model)?.label;
  const effortLabel = settings?.efforts.find((choice) => choice.id === settings.effort)?.label;
  const pillLabel = switching
    ? `${switching}…`
    : modelLabel
      ? effortLabel
        ? `${modelLabel} · ${effortLabel}`
        : modelLabel
      : (currentMode?.label ?? 'Mode');
  // A pick from the sheet or the mode menu: `act` has the desktop make it,
  // and `made` is what then stands in until the snapshot reports it.
  const pickWith = async (label: string, act: () => Promise<boolean>, made: Landed) => {
    if (switching || busy.current || conn?.state.status !== 'connected') return;
    setSwitching(label);
    setNote('');
    setSaid(null);
    await controlled(act, (why) => `Not switched: ${why}`, (text) => {
      setNote(text);
      setSaid({ text, failed: true });
    })
      .then((switched) => {
        if (!switched) return;
        setLanded((before) => ({ ...before, ...made }));
        setNote(`Switched to ${label}.`, 'done');
        setSaid({ text: `Switched to ${label}.`, failed: false });
      })
      .finally(() => setSwitching(''));
  };
  const pickModel = (model: string, effort: string, label: string) =>
    pickWith(
      label,
      () => conn?.setModel(terminalId, model, effort) ?? Promise.resolve(false),
      model ? { model: { to: model, from: told?.model ?? '' } } : { effort: { to: effort, from: told?.effort ?? '' } },
    );
  const pickMode = async (choice: { id: string; label: string }) => {
    setModeMenu(null);
    await pickWith(
      choice.label,
      () => conn?.setMode(terminalId, choice.id) ?? Promise.resolve(false),
      { mode: { to: choice.id, from: reported?.current ?? '' } },
    );
  };

  return {
    modes,
    settings,
    currentMode,
    modelLabel,
    effortLabel,
    pillLabel,
    switching,
    said,
    sheet,
    modeMenu,
    openSheet: () => {
      setSaid(null);
      setSheet(true);
    },
    closeSheet: () => setSheet(false),
    openMenu: (place: Place) => setModeMenu(place),
    closeMenu: () => setModeMenu(null),
    pickMode,
    pickModel,
  };
}

/** The pill in the chat's bar: the mode, or the model and effort, led by
 * the mode's glyph. It opens the sheet for an agent with models, and the
 * mode's menu, from where the pill is, for one without. Not there for an
 * agent the desktop can tell neither of. */
export function SessionPill({ picks, connected, busy }: { picks: SessionPicks; connected: boolean; busy: boolean }) {
  const pill = useRef<ViewInstance>(null);
  const { modes, settings, currentMode, modelLabel, effortLabel, switching } = picks;
  if (!modes && !settings) return null;
  return (
    <View ref={pill} collapsable={false}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={
          settings
            ? `${[currentMode?.label, modelLabel, effortLabel].filter(Boolean).join(', ') || 'Session settings'}. Change them`
            : currentMode
              ? `Mode: ${currentMode.label}. Change it`
              : 'Mode. Change it'
        }
        disabled={!connected || switching !== '' || (!settings && busy)}
        onPress={() => {
          if (settings) picks.openSheet();
          else placeOf(pill.current, picks.openMenu);
        }}
        style={({ pressed }) => [styles.pill, pressed && { backgroundColor: t.selection }]}
      >
        <ModeGlyph id={switching ? '' : (modes?.current ?? '')} />
        <Text style={[styles.pillText, settings && { maxWidth: 150 }]} numberOfLines={1}>
          {picks.pillLabel}
        </Text>
      </Pressable>
    </View>
  );
}

/** What the pill opens: the sheet, for an agent with models (1.26), or the
 * mode's menu, hung from the pill. `agent` is its name as a person reads it. */
export function SessionPickers({ picks, agent, busy }: { picks: SessionPicks; agent: string; busy: boolean }) {
  const { modes, settings } = picks;
  return (
    <>
      {settings ? (
        <ModelSheet
          open={picks.sheet}
          onClose={picks.closeSheet}
          agent={agent}
          modes={modes}
          settings={settings}
          pending={picks.switching}
          said={picks.said}
          busy={busy}
          onMode={(choice) => void picks.pickMode(choice)}
          onModel={(choice) => void picks.pickModel(choice.id, '', choice.label)}
          onEffort={(choice) => void picks.pickModel('', choice.id, choice.label)}
        />
      ) : null}
      <FloatMenu
        open={picks.modeMenu !== null}
        anchor={picks.modeMenu}
        onClose={picks.closeMenu}
        label={`${agent} mode`}
        sections={
          modes
            ? [
                {
                  title: `${agent} mode`,
                  items: modes.choices.map((choice) => ({
                    key: choice.id,
                    label: choice.label,
                    detail: choice.detail || undefined,
                    icon: choice.id === modes.current ? <Check size={16} color={t.brand} weight={2.4} /> : undefined,
                    onPress: () => void picks.pickMode(choice),
                  })),
                },
              ]
            : []
        }
      />
    </>
  );
}

/** The glyph a mode's pill leads with: pause for planning, a pencil for
 * edits going in, a shield for asking first, a bolt for going unasked. */
function ModeGlyph({ id }: { id: string }) {
  switch (id) {
    case 'plan':
      return <Pause size={15} color={t.attention} weight={2.6} />;
    case 'acceptEdits':
    case 'build':
      return <Pencil size={15} color={t.running} weight={2.2} />;
    case 'auto':
    case 'bypassPermissions':
    case 'dontAsk':
      return <Bolt size={15} color={t.brand} weight={2.2} />;
    case 'default':
      return <Shield size={15} color={t.dim} weight={2.2} />;
    default:
      return <Sliders size={15} color={t.dim} weight={2.2} />;
  }
}

const styles = StyleSheet.create({
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 34,
    paddingLeft: 10,
    paddingRight: 12,
    borderRadius: 17,
    borderWidth: 1,
    borderColor: t.border,
    backgroundColor: t.sunken,
  },
  pillText: { fontFamily: font.prose, fontSize: 13.5, fontWeight: '500', color: t.text, maxWidth: 104 },
});
