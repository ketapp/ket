'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// The quick prompt: the desktop's ⌘⇧P, floating over Worktrees. Say what to
// do, and ket on the desktop makes a worktree for it and starts an agent, in
// the project and with the agent picked under the field — the ones last
// started with on this desktop, so most tasks are typed and sent untouched.
//
// It is the short way. New task (the sheet) is the long one: it can start in
// an existing worktree, on another base, with a model and an effort.

import { useEffect, useRef, useState } from 'react';
import { Keyboard, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import { FloatMenu, type MenuItem, type Place, placeOf } from './FloatMenu';
import { KeyCommands } from './KeyCommands';
import { AgentMark, Check, ChevronDown, Send } from './icons';
import { Badge } from './ui';
import { projectColor } from '../lib/activity';
import type { HostConnection } from '../lib/connection';
import { getLastWork, saveLastWork } from '../lib/store';
import { agentLabel, font, size, t } from '../lib/theme';

type Which = 'project' | 'agent';

export function QuickPrompt({ conn }: { conn: HostConnection }) {
  const hostId = conn.host.id;
  // Not "Elsewhere": the terminals the desktop could not place are no
  // project to start work in.
  const projects = (conn.snapshot?.projects ?? []).filter((project) => project.id !== '');
  const agents = conn.snapshot?.agents ?? [];
  const [picked, setPicked] = useState<string | null>(null);
  // Empty for the project's own agent.
  const [wanted, setWanted] = useState('');
  const [prompt, setPrompt] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const [menu, setMenu] = useState<{ which: Which; place: Place } | null>(null);
  const pills = useRef<Partial<Record<Which, View | null>>>({});
  // A pick made here wins over the one remembered, however late that loads.
  const touched = useRef(false);

  useEffect(() => {
    touched.current = false;
    void getLastWork(hostId).then((last) => {
      if (!last || touched.current) return;
      setPicked(last.project);
      setWanted(last.agent);
    });
  }, [hostId]);

  const project = projects.find((p) => p.id === picked) ?? projects[0];
  const agent = agents.includes(wanted) ? wanted : '';
  const ready = prompt.trim() !== '' && project !== undefined && !sending;

  const openMenu = (which: Which) => {
    Keyboard.dismiss();
    placeOf(pills.current[which], (place) => setMenu({ which, place }));
  };

  const send = async () => {
    if (!ready || !project) return;
    setSending(true);
    setError('');
    const why = await conn
      .startWork(project.id, prompt.trim(), agent)
      .catch((e: unknown) => (e instanceof Error ? e.message : String(e)));
    setSending(false);
    if (why) {
      setError(why.charAt(0).toUpperCase() + why.slice(1));
      return;
    }
    setPrompt('');
    Keyboard.dismiss();
    void saveLastWork(hostId, { project: project.id, agent });
  };

  const items: MenuItem[] =
    menu?.which === 'project'
      ? projects.map((p) => ({
          key: p.id,
          label: p.name,
          icon: p.id === project?.id ? <Check size={16} color={t.text} /> : undefined,
          onPress: () => {
            touched.current = true;
            setPicked(p.id);
          },
        }))
      : ['', ...agents].map((name) => ({
          key: name || 'default',
          label: name ? agentLabel(name) : "Project's own",
          icon: name === agent ? <Check size={16} color={t.text} /> : undefined,
          onPress: () => {
            touched.current = true;
            setWanted(name);
          },
        }));

  return (
    <View style={styles.composer}>
      <KeyCommands title="Start task" onSend={() => void send()}>
        <TextInput
          value={prompt}
          onChangeText={(text) => {
            setPrompt(text);
            if (error) setError('');
          }}
          placeholder="Start an agent on something…"
          placeholderTextColor={t.faint}
          accessibilityLabel="Quick prompt"
          multiline
          style={styles.field}
        />
      </KeyCommands>
      {error ? (
        <Text style={styles.error} numberOfLines={3}>
          {error}
        </Text>
      ) : null}
      <View style={styles.foot}>
        <Text style={styles.word}>in</Text>
        <Pressable
          ref={(view) => {
            pills.current.project = view;
          }}
          accessibilityRole="button"
          accessibilityLabel={`Project: ${project?.name ?? 'none'}`}
          disabled={projects.length < 2}
          onPress={() => openMenu('project')}
          style={({ pressed }) => [styles.pill, pressed && { backgroundColor: t.selection }]}
        >
          {project ? <Badge name={project.name} color={projectColor(project)} size={16} /> : null}
          <Text style={styles.pillText} numberOfLines={1}>
            {project?.name ?? 'No projects'}
          </Text>
          {projects.length > 1 ? <ChevronDown size={12} color={t.dim} weight={2} /> : null}
        </Pressable>
        {agents.length > 0 ? (
          <>
            <Text style={styles.word}>with</Text>
            <Pressable
              ref={(view) => {
                pills.current.agent = view;
              }}
              accessibilityRole="button"
              accessibilityLabel={`Agent: ${agent ? agentLabel(agent) : "Project's own"}`}
              onPress={() => openMenu('agent')}
              style={({ pressed }) => [styles.pill, styles.shrink, pressed && { backgroundColor: t.selection }]}
            >
              {agent ? <AgentMark agent={agent} size={12} /> : null}
              <Text style={styles.pillText} numberOfLines={1}>
                {agent ? agentLabel(agent) : 'default'}
              </Text>
              <ChevronDown size={12} color={t.dim} weight={2} />
            </Pressable>
          </>
        ) : null}
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Start task"
          accessibilityState={{ disabled: !ready }}
          disabled={!ready}
          onPress={() => void send()}
          style={({ pressed }) => [styles.send, !ready && styles.sendIdle, pressed && { backgroundColor: t.accentPressed }]}
        >
          <Send size={16} color={ready ? t.onAccent : t.faint} weight={2.2} />
        </Pressable>
      </View>
      <FloatMenu
        open={menu !== null}
        onClose={() => setMenu(null)}
        anchor={menu?.place ?? null}
        label={menu?.which === 'project' ? 'Projects' : 'Agents'}
        width={240}
        sections={[{ items }]}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  composer: {
    borderRadius: 14,
    backgroundColor: t.elevated,
    borderWidth: 1,
    borderColor: t.border,
    shadowColor: '#000',
    shadowOpacity: 0.55,
    shadowRadius: 18,
    shadowOffset: { width: 0, height: 12 },
  },
  field: {
    maxHeight: 112,
    paddingHorizontal: 14,
    paddingTop: 12,
    paddingBottom: 4,
    color: t.text,
    fontFamily: font.prose,
    fontSize: size.body,
    lineHeight: 20,
  },
  error: { paddingHorizontal: 14, paddingTop: 4, fontFamily: font.prose, fontSize: 12, color: t.failed },
  foot: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingLeft: 14, paddingRight: 6, paddingVertical: 6 },
  word: { fontFamily: font.prose, fontSize: 12, color: t.dim },
  pill: {
    height: 30,
    maxWidth: 130,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    paddingHorizontal: 8,
    borderRadius: size.radiusSm,
    borderWidth: 1,
    borderColor: t.border,
  },
  shrink: { flexShrink: 1 },
  pillText: { flexShrink: 1, fontFamily: font.chrome, fontSize: 11.5, color: t.text },
  send: {
    marginLeft: 'auto',
    width: 38,
    height: 38,
    borderRadius: 9,
    backgroundColor: t.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  sendIdle: { backgroundColor: t.sunken },
});
