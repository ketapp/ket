'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// New work from the phone, as a sheet over Worktrees: pick a project and an
// agent from two dropdowns, say what to do, and ket on the desktop makes a
// worktree for it and starts the agent on it — the project's own, or one
// picked here from those the desktop can run — the same as its own Create
// Worktree dialog. It shows up on Worktrees, behind the sheet, a moment later.
//
// The project and agent last started with on this desktop come back next
// time, so most tasks are typed and started without touching either.

import { router, useLocalSearchParams } from 'expo-router';
import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  Dimensions,
  Keyboard,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import Animated, {
  Easing,
  useAnimatedStyle,
  useSharedValue,
  withSpring,
  withTiming,
} from 'react-native-reanimated';
import { KeyboardStickyView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { AgentMark, Check, ChevronDown, Close } from '../../components/icons';
import { Badge, Button, Prose } from '../../components/ui';
import { projectColor } from '../../lib/activity';
import { useHost } from '../../lib/hosts';
import { getLastWork, saveLastWork } from '../../lib/store';
import { agentLabel, font, size, t } from '../../lib/theme';

/** Which dropdown is open. */
type Which = 'project' | 'agent';

/** Where a dropdown's field sits in the sheet, for its menu to open above. */
interface Place {
  x: number;
  width: number;
}

export default function NewTask() {
  const { host: hostId } = useLocalSearchParams<{ host: string }>();
  const conn = useHost(hostId);
  const insets = useSafeAreaInsets();
  // Projects the desktop lists, without "Elsewhere" — the terminals it could
  // not place, which are no project to start work in.
  const projects = (conn?.snapshot?.projects ?? []).filter((project) => project.id !== '');
  // The agents the desktop can start; none from a desktop too old to say.
  const agents = conn?.snapshot?.agents ?? [];
  const [picked, setPicked] = useState<string | null>(null);
  // Empty for the project's own agent.
  const [wanted, setWanted] = useState('');
  const [prompt, setPrompt] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const [open, setOpen] = useState<Which | null>(null);
  const [typing, setTyping] = useState(() => Keyboard.isVisible());
  // Where the menus open from: the fields, the row they sit in, the sheet.
  const [places, setPlaces] = useState<Partial<Record<Which, Place>>>({});
  const [rowY, setRowY] = useState(0);
  const [sheetH, setSheetH] = useState(0);
  const [room, setRoom] = useState({ width: 0, height: 0 });
  // A pick made here wins over the one remembered, however late that loads.
  const touched = useRef(false);
  // The sheet sticks to the keyboard, frame for frame, every time it moves —
  // react-native-keyboard-controller, as the session screen does. Its foot
  // keeps the home indicator's room throughout: the keyboard covers that
  // room first, and the sheet only rises for the rest.
  const foot = Math.max(insets.bottom, 12);
  const tucked = foot - 12;
  // Presented as iOS presents a sheet: the dimming fades in while the sheet
  // springs up from below the screen, and both go back the way they came.
  // Drawn here rather than by the stack, which only knows how to fade or
  // slide the whole screen — dimming and all.
  const dim = useSharedValue(0);
  const drop = useSharedValue(Dimensions.get('window').height);
  const entered = useRef(false);
  const leaving = useRef(false);
  const dimming = useAnimatedStyle(() => ({ opacity: dim.get() }));
  const sliding = useAnimatedStyle(() => ({ transform: [{ translateY: drop.get() }] }));

  useEffect(() => {
    dim.set(withTiming(1, { duration: DIM_MS }));
  }, [dim]);

  useEffect(() => {
    void getLastWork(hostId).then((last) => {
      if (!last || touched.current) return;
      setPicked(last.project);
      setWanted(last.agent);
    });
  }, [hostId]);

  // From the commit, not after paint: the prompt focuses itself as it mounts,
  // and the keyboard's first event must not arrive before anyone listens.
  useLayoutEffect(() => {
    const show = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow', () => setTyping(true));
    const hide = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide', () => setTyping(false));
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  const project = projects.find((p) => p.id === picked) ?? projects[0];
  const agent = agents.includes(wanted) ? wanted : '';
  const choosesAgent = agents.length > 1;
  // A desktop from before 1.7 can't take new work.
  const tooOld = conn !== null && conn.status === 'connected' && conn.hostMinor < 7;

  // Down and out first, then off the stack.
  const close = () => {
    if (leaving.current) return;
    leaving.current = true;
    Keyboard.dismiss();
    dim.set(withTiming(0, { duration: LEAVE_MS }));
    drop.set(withTiming(sheetH || Dimensions.get('window').height, { duration: LEAVE_MS, easing: Easing.in(Easing.cubic) }));
    setTimeout(() => (router.canGoBack() ? router.back() : router.replace('/')), LEAVE_MS);
  };

  // Up from just below the screen, once the sheet knows how tall it is.
  const arrive = (height: number) => {
    setSheetH(height);
    if (entered.current) return;
    entered.current = true;
    drop.set(height);
    drop.set(withSpring(0, ARRIVE));
  };

  // A menu needs the room the keyboard is using, so it goes first.
  const toggle = (which: Which) => {
    Keyboard.dismiss();
    setOpen(open === which ? null : which);
  };

  const pickProject = (id: string) => {
    touched.current = true;
    setPicked(id);
    setOpen(null);
  };

  const pickAgent = (name: string) => {
    touched.current = true;
    setWanted(name);
    setOpen(null);
  };

  const start = async () => {
    if (!conn || !project || !prompt.trim()) return;
    setSending(true);
    setError('');
    const why = await conn.startWork(project.id, prompt.trim(), agent).catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setSending(false);
    if (why) {
      setError(why.charAt(0).toUpperCase() + why.slice(1));
      return;
    }
    void saveLastWork(hostId, { project: project.id, agent });
    close();
  };

  const placed = (which: Which) => (e: { nativeEvent: { layout: { x: number; width: number } } }) => {
    const { x, width } = e.nativeEvent.layout;
    setPlaces((was) => ({ ...was, [which]: { x, width } }));
  };

  // The menu opens upward from the field, as far as the room above it goes.
  const place = open ? places[open] : undefined;
  const menuBottom = sheetH - rowY + 8;
  const menuRoom = Math.max(160, room.height - menuBottom - insets.top - 12);
  const menuWidth = Math.max(place?.width ?? 0, 230);
  // Under its field, but never past the screen's right edge.
  const menuLeft = Math.min(place?.x ?? 12, room.width - 12 - menuWidth);

  return (
    <View style={styles.fill}>
      {/* With the keyboard up, a tap above the sheet only puts it away; the
          next one closes. */}
      <Animated.View style={[styles.scrim, dimming]}>
        <Pressable
          accessibilityLabel={typing ? 'Hide the keyboard' : 'Close New task'}
          onPress={() => (typing ? Keyboard.dismiss() : close())}
          style={styles.fill}
        />
      </Animated.View>
      <View style={styles.fill} pointerEvents="box-none">
        <View style={styles.room} pointerEvents="box-none" onLayout={(e) => setRoom({ width: e.nativeEvent.layout.width, height: e.nativeEvent.layout.height })}>
          <KeyboardStickyView offset={{ opened: tucked }}>
            <Animated.View
              style={[styles.sheet, { paddingBottom: foot }, sliding]}
              onLayout={(e) => arrive(e.nativeEvent.layout.height)}
            >
              <View style={styles.grabber} />
              <View style={styles.head}>
                <View style={styles.headText}>
                  <Text style={styles.title}>New task</Text>
                  {conn ? (
                    <Text style={styles.host} numberOfLines={1}>
                      {conn.host.name}
                    </Text>
                  ) : null}
                </View>
                <Pressable accessibilityRole="button" accessibilityLabel="Close" onPress={close} hitSlop={8} style={styles.close}>
                  <Close size={16} color={t.dim} weight={2} />
                </Pressable>
              </View>

              <View style={styles.fields} onLayout={(e) => setRowY(e.nativeEvent.layout.y)}>
                <Field
                  label="Project"
                  open={open === 'project'}
                  disabled={projects.length === 0}
                  onPress={() => toggle('project')}
                  onLayout={placed('project')}
                  lead={project ? <Badge name={project.name} color={projectColor(project)} size={18} /> : null}
                  value={project?.name ?? 'No projects'}
                />
                {choosesAgent ? (
                  <Field
                    label="Agent"
                    open={open === 'agent'}
                    onPress={() => toggle('agent')}
                    onLayout={placed('agent')}
                    lead={<AgentMark agent={agent} size={15} />}
                    value={agent ? agentLabel(agent) : 'Default'}
                  />
                ) : null}
              </View>

              <TextInput
                value={prompt}
                onChangeText={setPrompt}
                onFocus={() => setOpen(null)}
                placeholder="What should the agent do?"
                placeholderTextColor={t.faint}
                multiline
                autoFocus
                style={styles.prompt}
              />

              {projects.length === 0 ? (
                <Prose style={styles.note}>No projects on this desktop yet.</Prose>
              ) : null}
              {tooOld ? (
                <Prose style={styles.note} color={t.failed}>
                  ket on {conn?.host.name ?? 'your Mac'} is older than this app. Update it — rebuild ket and start a new host —
                  to start work from here.
                </Prose>
              ) : null}
              {error ? (
                <Prose style={styles.note} color={t.failed}>
                  {error}
                </Prose>
              ) : null}

              <View style={styles.actions}>
                <Button
                  kind="brand"
                  title={sending ? 'Starting…' : 'Start task'}
                  disabled={sending || tooOld || !project || !prompt.trim()}
                  onPress={() => void start()}
                />
              </View>
            </Animated.View>
          </KeyboardStickyView>

          {open && place ? (
            <>
              <Pressable accessibilityLabel="Close menu" onPress={() => setOpen(null)} style={StyleSheet.absoluteFill} />
              <View
                accessibilityRole="menu"
                style={[styles.menu, { left: menuLeft, width: menuWidth, bottom: menuBottom, maxHeight: menuRoom }]}
              >
                <ScrollView bounces={false} keyboardShouldPersistTaps="handled">
                  {open === 'project' ? (
                    <>
                      <Text style={styles.menuHeading}>Projects on {conn?.host.name ?? 'this desktop'}</Text>
                      {projects.map((p) => (
                        <Item
                          key={p.id}
                          on={p.id === project?.id}
                          lead={<Badge name={p.name} color={projectColor(p)} size={20} />}
                          label={p.name}
                          onPress={() => pickProject(p.id)}
                        />
                      ))}
                    </>
                  ) : (
                    <>
                      <Item
                        on={agent === ''}
                        lead={<AgentMark agent="" size={16} />}
                        label="Default"
                        note={project ? `Whatever ket uses for ${project.name}` : 'Whatever ket uses for the project'}
                        onPress={() => pickAgent('')}
                      />
                      {agents.map((name) => (
                        <Item
                          key={name}
                          on={agent === name}
                          lead={<AgentMark agent={name} size={16} />}
                          label={agentLabel(name)}
                          onPress={() => pickAgent(name)}
                        />
                      ))}
                    </>
                  )}
                </ScrollView>
              </View>
            </>
          ) : null}
        </View>
      </View>
    </View>
  );
}

/** The dimming's fade in. */
const DIM_MS = 300;

/** The sheet's way in: critically damped, as UIKit springs a sheet up. */
const ARRIVE = { duration: 450, dampingRatio: 1 } as const;

/** The way out, sheet and dimming together. */
const LEAVE_MS = 260;

/** A dropdown's field: its label over what is chosen, and a chevron. */
function Field({
  label,
  lead,
  value,
  open,
  disabled,
  onPress,
  onLayout,
}: {
  label: string;
  lead: ReactNode;
  value: string;
  open: boolean;
  disabled?: boolean;
  onPress: () => void;
  onLayout: (e: { nativeEvent: { layout: { x: number; width: number } } }) => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${label}: ${value}`}
      accessibilityState={{ expanded: open, disabled }}
      disabled={disabled}
      onPress={onPress}
      onLayout={onLayout}
      style={({ pressed }) => [styles.field, (open || pressed) && styles.fieldOpen, disabled && { opacity: 0.55 }]}
    >
      <Text style={styles.fieldLabel}>{label}</Text>
      <View style={styles.fieldValue}>
        {lead}
        <Text style={styles.fieldText} numberOfLines={1}>
          {value}
        </Text>
        <ChevronDown size={15} color={t.dim} weight={2.2} />
      </View>
    </Pressable>
  );
}

/** One choice in a dropdown's menu, ticked when it is the one chosen. */
function Item({ on, lead, label, note, onPress }: { on: boolean; lead: ReactNode; label: string; note?: string; onPress: () => void }) {
  return (
    <Pressable
      accessibilityRole="menuitem"
      accessibilityState={{ selected: on }}
      onPress={onPress}
      style={({ pressed }) => [styles.item, note ? styles.itemTall : null, (on || pressed) && { backgroundColor: t.selection }]}
    >
      {lead}
      <View style={styles.itemText}>
        <Text style={styles.itemLabel} numberOfLines={1}>
          {label}
        </Text>
        {note ? (
          <Text style={styles.itemNote} numberOfLines={1}>
            {note}
          </Text>
        ) : null}
      </View>
      {on ? <Check size={16} color={t.text} weight={2.2} /> : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  scrim: { position: 'absolute', top: 0, right: 0, bottom: 0, left: 0, backgroundColor: 'rgba(0,0,0,0.6)' },
  room: { flex: 1, justifyContent: 'flex-end' },
  sheet: {
    backgroundColor: t.card,
    borderTopLeftRadius: 18,
    borderTopRightRadius: 18,
    borderTopWidth: 1,
    borderColor: t.border,
    shadowColor: '#000',
    shadowOpacity: 0.55,
    shadowRadius: 25,
    shadowOffset: { width: 0, height: -20 },
  },
  grabber: { width: 36, height: 5, borderRadius: 3, backgroundColor: t.border, alignSelf: 'center', marginTop: 8, marginBottom: 6 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingTop: 4, paddingBottom: 14, paddingLeft: 16, paddingRight: 12 },
  headText: { flex: 1, minWidth: 0, gap: 1 },
  title: { fontFamily: font.proseSemibold, fontSize: size.value, color: t.text },
  host: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  close: { width: 32, height: 32, borderRadius: 16, backgroundColor: t.sunken, alignItems: 'center', justifyContent: 'center' },
  fields: { flexDirection: 'row', gap: 8, paddingHorizontal: 12 },
  field: {
    flex: 1,
    minWidth: 0,
    height: 56,
    borderRadius: size.radiusLg,
    backgroundColor: t.sunken,
    borderWidth: 1,
    borderColor: 'transparent',
    paddingVertical: 8,
    paddingLeft: 12,
    paddingRight: 10,
    justifyContent: 'center',
    gap: 4,
  },
  fieldOpen: { backgroundColor: t.selection, borderColor: t.dim },
  fieldLabel: { fontFamily: font.prose, fontSize: 11.5, color: t.dim },
  fieldValue: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  fieldText: { flex: 1, minWidth: 0, fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  prompt: {
    marginTop: 8,
    marginHorizontal: 12,
    height: 112,
    borderRadius: size.radiusLg,
    backgroundColor: t.sunken,
    padding: 12,
    color: t.text,
    fontFamily: font.prose,
    fontSize: 15,
    lineHeight: 22,
    textAlignVertical: 'top',
  },
  note: { paddingHorizontal: 16, paddingTop: 8, fontSize: size.detail },
  actions: { paddingTop: 10, paddingHorizontal: 12 },
  menu: {
    position: 'absolute',
    padding: 4,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
    borderRadius: size.radiusCard,
    shadowColor: '#000',
    shadowOpacity: 0.6,
    shadowRadius: 20,
    shadowOffset: { width: 0, height: 18 },
    elevation: 12,
  },
  menuHeading: { paddingTop: 8, paddingBottom: 4, paddingHorizontal: 12, fontFamily: font.proseMedium, fontSize: 11.5, color: t.faint },
  item: { height: 44, flexDirection: 'row', alignItems: 'center', gap: 11, paddingHorizontal: 12, borderRadius: size.radius },
  itemTall: { height: 52 },
  itemText: { flex: 1, minWidth: 0, gap: 1 },
  itemLabel: { fontFamily: font.proseMedium, fontSize: size.body, color: t.text },
  itemNote: { fontFamily: font.prose, fontSize: 11.5, color: t.dim },
});
