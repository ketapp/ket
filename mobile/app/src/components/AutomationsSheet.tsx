// A Mac's automations — work that starts itself on a schedule — as a sheet:
// each with its next start, a switch to pause it, and a tap for Run now,
// Pause and Delete. Making or editing one is the Mac's: its New worktree
// dialog's Schedule…, or its Automations dialog.

import { AutomationEdit, type AutomationSummary } from '@ket/remote';
import { useEffect, useState } from 'react';
import { Alert, Pressable, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import type { HostConnection } from '../lib/connection';
import { t } from '../lib/theme';
import { type MenuSection, SheetMenu } from './FloatMenu';
import { Clock, Pause, Play, Trash } from './icons';
import { SHEET_HEAD, SHEET_INSET, Sheet, SheetItem, SheetList, type SheetMeasure, SheetNote } from './Sheet';
import { Toggle } from './ui';

/** `Today 2:00 AM`, `Tomorrow 9:00 AM`, `Fri 20 Nov 6:00 PM`. */
export function startLabel(ms: number, now: number = Date.now()): string {
  const at = new Date(ms);
  const time = at.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((day(at) - day(new Date(now))) / 86_400_000);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Tomorrow ${time}`;
  const date = at.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
  return `${date} ${time}`;
}

/** What a row says under its name. */
export function rowDetail(a: AutomationSummary, now: number = Date.now()): string {
  const when = a.nextStartMs > 0n ? `Next ${startLabel(Number(a.nextStartMs), now)}` : a.finished ? 'Done' : 'Paused';
  return `${when} · ${a.schedule}`;
}

/** The colour of a row's dot: its last run's result. */
function resultColor(result: string): string | undefined {
  switch (result) {
    case 'Done':
    case 'Running':
      return t.running;
    case 'Failed':
    case 'Couldn’t start':
      return t.failed;
    case '':
      return undefined;
    default:
      return t.faint;
  }
}

export function AutomationsSheet({
  open,
  onClose,
  conn,
  hostName,
}: {
  open: boolean;
  onClose: () => void;
  conn: HostConnection | null;
  hostName: string;
}) {
  // What a Mac said, kept with the Mac that said it: after a switch, the
  // last Mac's rows are never shown, nor acted on, under the next one's name.
  const [got, setGot] = useState<{ from: HostConnection; list: AutomationSummary[] | null; problem: string | null } | null>(
    null,
  );
  const mine = got !== null && got.from === conn ? got : null;
  const list = mine?.list ?? null;
  const problem = mine?.problem ?? null;
  // The row a menu is open on, by id; where each row and the list lie in the
  // sheet's body, for the menu to rise from its row.
  const [menuId, setMenuId] = useState<string | null>(null);
  const [rowYs, setRowYs] = useState<Record<string, number>>({});
  const [listY, setListY] = useState(0);
  const [measure, setMeasure] = useState<SheetMeasure | null>(null);
  const insets = useSafeAreaInsets();
  const menu = menuId ? (list?.find((a) => a.id === menuId) ?? null) : null;

  useEffect(() => {
    if (!open || !conn) return;
    let live = true;
    conn
      .automations()
      .then((list) => live && setGot({ from: conn, list, problem: null }))
      .catch((e: unknown) => live && setGot({ from: conn, list: null, problem: e instanceof Error ? e.message : String(e) }));
    return () => {
      live = false;
    };
  }, [open, conn]);

  const edit = (automation: AutomationSummary, how: AutomationEdit) => {
    if (!conn) return;
    setMenuId(null);
    const from = conn;
    from
      .editAutomation(automation.id, how)
      .then((list) => setGot({ from, list, problem: null }))
      .catch((e: unknown) => Alert.alert('Couldn’t change it', e instanceof Error ? e.message : String(e)));
  };

  const sections: MenuSection[] = menu
    ? [
        {
          items: [
            {
              key: 'run',
              label: 'Run now',
              mark: <Play size={16} color={t.text} />,
              detail: 'Starts it on the Mac, whatever its schedule',
              onPress: () => edit(menu, AutomationEdit.RUN_NOW),
            },
            menu.enabled
              ? {
                  key: 'pause',
                  label: 'Pause',
                  mark: <Pause size={16} color={t.text} />,
                  onPress: () => edit(menu, AutomationEdit.PAUSE),
                }
              : {
                  key: 'resume',
                  label: 'Turn back on',
                  mark: <Clock size={16} color={t.text} />,
                  onPress: () => edit(menu, AutomationEdit.RESUME),
                },
          ],
        },
        {
          items: [
            {
              key: 'delete',
              label: 'Delete',
              danger: true,
              mark: <Trash size={16} color={t.failed} />,
              onPress: () => {
                const chosen = menu;
                setMenuId(null);
                Alert.alert(`Delete “${chosen.name}”?`, 'Its history goes too. The worktrees its runs made stay.', [
                  { text: 'Keep', style: 'cancel' },
                  { text: 'Delete', style: 'destructive', onPress: () => edit(chosen, AutomationEdit.DELETE) },
                ]);
              },
            },
          ],
        },
      ]
    : [];

  // Up from the top of its row, as the New task sheet's menus open, and
  // never past the screen's right edge.
  const room = measure?.room ?? { width: 0, height: 0 };
  const menuWidth = 260;
  const menuBottom = (measure?.height ?? 0) - SHEET_HEAD - listY - (menu ? (rowYs[menu.id] ?? 0) : 0) + 8;
  const menuRoom = Math.max(160, room.height - menuBottom - insets.top - 12);
  const menuLeft = Math.max(12, room.width - SHEET_INSET - menuWidth);

  return (
    <Sheet
      open={open}
      onClose={() => {
        setMenuId(null);
        onClose();
      }}
      title="Automations"
      detail={`On ${hostName}`}
      onMeasure={setMeasure}
      overlay={
        menu && measure ? (
          <>
            <Pressable accessibilityLabel="Close menu" onPress={() => setMenuId(null)} style={StyleSheet.absoluteFill} />
            <SheetMenu
              rise={measure.rise}
              bottom={menuBottom}
              left={menuLeft}
              width={menuWidth}
              maxHeight={menuRoom}
              sections={sections}
              label={`${menu.name} actions`}
            />
          </>
        ) : null
      }
    >
      {list && list.length > 0 ? (
        <View onLayout={(e) => setListY(e.nativeEvent.layout.y)}>
          <SheetList>
            {list.map((a) => (
              <View
                key={a.id}
                onLayout={(e) => {
                  const y = e.nativeEvent.layout.y;
                  setRowYs((was) => (was[a.id] === y ? was : { ...was, [a.id]: y }));
                }}
              >
                <SheetItem
                  label={a.name}
                  mark={<Clock size={18} color={a.enabled && !a.finished ? t.text : t.faint} />}
                  detail={rowDetail(a)}
                  dot={resultColor(a.lastResult)}
                  icon={
                    a.finished ? undefined : (
                      <Toggle
                        on={a.enabled}
                        label={`${a.name} on`}
                        onChange={(on) => edit(a, on ? AutomationEdit.RESUME : AutomationEdit.PAUSE)}
                      />
                    )
                  }
                  onPress={() => setMenuId(a.id)}
                />
              </View>
            ))}
          </SheetList>
        </View>
      ) : null}
      {problem ? <SheetNote>{problem}</SheetNote> : null}
      {list && list.length === 0 ? <SheetNote>Nothing is scheduled on {hostName}.</SheetNote> : null}
      <SheetNote>New automations are made on the Mac: Start ▾ › Schedule… in its New worktree dialog.</SheetNote>
    </Sheet>
  );
}
