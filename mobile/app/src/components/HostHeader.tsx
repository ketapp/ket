// The head of a tab that shows one desktop — Worktrees, Usage: the desktop's
// name as the screen's large title, a tap opening the Macs sheet to switch;
// under it the lock on the line to it and how it runs, a tap saying what
// that means; across from them, Start, which opens the New task sheet. Each
// of those tabs draws the same head, so the top of the screen stays put as
// you move between them.

import { useState } from 'react';
import { Alert, Pressable, StyleSheet, Text, View } from 'react-native';

import type { AddressKind } from '../lib/address-kind';
import type { HostConnection } from '../lib/connection';
import { type PairedHost, statusLine } from '../lib/hosts';
import { font, size, t } from '../lib/theme';
import { AutomationsSheet } from './AutomationsSheet';
import { Chevron, Clock, Desktop, Plus, Sparkle } from './icons';
import { Seal } from './Seal';
import { Sheet, SheetItem, SheetList, SheetNote } from './Sheet';
import { Masthead } from './ui';
import { PRESSED } from './ui/surface';
import { shared } from './ui/shared';

/** How the link to the desktop runs, after "Secure" under its name. */
const ROUTE_WORD: Record<AddressKind, string> = {
  local: 'Wi-Fi',
  tailnet: 'Tailscale',
  'ket-relay': 'Relay',
  relay: 'Relay',
};

export function HostHeader({
  hosts,
  current,
  onChoose,
  onStart,
}: {
  /** Every paired desktop, for the Macs sheet. */
  hosts: readonly PairedHost[] | null;
  /** The desktop the screen shows. */
  current: PairedHost | null;
  onChoose: (conn: HostConnection) => void;
  /** Start, on a desktop that is connected. */
  onStart: (conn: HostConnection) => void;
}) {
  const conn = current?.conn ?? null;
  const hostState = current?.hostState ?? null;
  // The Mac switcher, a sheet, while it is up.
  const [picking, setPicking] = useState(false);
  const [automating, setAutomating] = useState(false);
  // Automations, asked for from the Macs sheet: it rises once that sheet's
  // modal is gone, since iOS shows one at a time.
  const [toAutomations, setToAutomations] = useState(false);
  const status = statusLine(hostState);
  const statusWords = status.connected
    ? status.kind
      ? `Secure · ${ROUTE_WORD[status.kind]}`
      : 'Secure'
    : hostState?.status === 'offline'
      ? 'Offline'
      : 'Reaching';
  const explainSecure = () =>
    Alert.alert(
      'End-to-end encrypted',
      `Everything between this phone and ${hostState?.host.name || 'your desktop'} is end-to-end encrypted. The relay only ever sees ciphertext.${status.kind ? `\n\n${status.text}.` : ''}`,
    );

  return (
    <>
      <Masthead
        title={hostState?.host.name ?? ''}
        titleLabel="Switch desktop"
        onTitle={() => setPicking(true)}
        line={
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${status.connected ? 'Secure: end-to-end encrypted' : statusWords}${status.kind ? `, ${status.text}` : ''}`}
            onPress={explainSecure}
            hitSlop={6}
            style={({ pressed }) => [styles.status, pressed && { opacity: PRESSED }]}
          >
            <Seal sealed={status.connected} size={11} />
            <Text style={shared.headLine} numberOfLines={1}>
              {statusWords}
            </Text>
          </Pressable>
        }
        action={
          conn && status.connected ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Start a task"
              onPress={() => onStart(conn)}
              style={({ pressed }) => [styles.new, pressed && { backgroundColor: '#9aeaa4' }]}
            >
              <Sparkle size={15} color={t.onBrand} weight={2.2} />
              <Text style={styles.newText}>Start</Text>
            </Pressable>
          ) : null
        }
      />

      {/* Every paired Mac, and a way to add one — offered with a single Mac
          too, so the switcher says it can hold more. Adding one is not wired
          yet. */}
      <Sheet
        open={picking}
        onClose={() => setPicking(false)}
        onGone={() => {
          if (!toAutomations) return;
          setToAutomations(false);
          setAutomating(true);
        }}
        title="Macs"
        detail="Paired with this device"
      >
        <SheetList>
          {(hosts ?? []).map(({ conn: c, hostState: h }) => (
            <SheetItem
              key={c.id}
              role="radio"
              label={h.host.name}
              mono
              mark={<Desktop size={18} color={t.text} />}
              detail={statusLine(h).text}
              dot={h.status === 'connected' ? t.running : h.status === 'offline' ? t.failed : t.faint}
              on={c === conn}
              onPress={() => {
                setPicking(false);
                onChoose(c);
              }}
            />
          ))}
        </SheetList>
        {conn && hostState && hostState.hostMinor >= 36 ? (
          <SheetList>
            <SheetItem
              label="Automations"
              mark={<Clock size={18} color={t.text} />}
              detail={`Work that starts itself on ${hostState.host.name}: pause one, or start it now.`}
              icon={<Chevron size={12} color={t.dim} weight={2.4} />}
              onPress={() => {
                setToAutomations(true);
                setPicking(false);
              }}
            />
          </SheetList>
        ) : null}
        <SheetList>
          <SheetItem
            label="Add another Mac"
            mark={
              <View style={styles.addMark}>
                <Plus size={10} color={t.text} weight={3} />
              </View>
            }
            detail="Pair ket on a second Mac, then switch between their agents here."
            icon={<Chevron size={12} color={t.dim} weight={2.4} />}
            onPress={() => {}}
          />
        </SheetList>
        <SheetNote>On that Mac: Settings › Devices › Pair a device, then scan its code here.</SheetNote>
      </Sheet>
      <AutomationsSheet
        open={automating}
        onClose={() => setAutomating(false)}
        conn={conn}
        hostName={hostState?.host.name ?? 'your Mac'}
      />
    </>
  );
}

const styles = StyleSheet.create({
  // The lock on the line to the desktop, and how it runs.
  status: { alignSelf: 'flex-start', flexDirection: 'row', alignItems: 'center', gap: 5 },
  // The empty slot a Mac would fill: a dashed square with a plus in it.
  addMark: {
    width: 18,
    height: 18,
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t.faint,
    alignItems: 'center',
    justifyContent: 'center',
  },
  // The one way to start work from here, so it wears ket's green — the
  // colour Send fills with when there is something to send.
  new: {
    height: 40,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingLeft: 13,
    paddingRight: 16,
    borderRadius: 20,
    backgroundColor: t.brand,
  },
  newText: { fontFamily: font.proseSemibold, fontSize: size.label, color: t.onBrand },
});
