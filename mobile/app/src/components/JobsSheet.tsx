// What a chat's agent left running in the background on the Mac (protocol
// 1.40) — a build, a test run, a dev server — and what of that ended a moment
// ago. The chat's capsule (`ChatJobs`) opens this; each running job has its
// Stop, which ends it and everything it started on the Mac.
//
// A server shows its port and no way to open it: it is on the Mac's own
// localhost, which the phone cannot reach.

import { useState } from 'react';
import { StyleSheet, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { type Job, JobEnded_Outcome } from '@ket/remote';

import { compactDuration, useNow } from '../lib/usage';
import { agentLabel, t } from '../lib/theme';
import { Check, Close, Globe, Loader, Stop } from './icons';
import { Sheet, SheetItem, SheetList, SheetNote } from './Sheet';
import { Pill, SectionLabel } from './ui';

/** A job by what tells it from any other: its process and when it started. */
function keyOf(job: Job): string {
  return `${job.pid}:${job.startedAtMs.toString()}`;
}

/** What a job runs, on one line. */
function commandOf(job: Job): string {
  return job.command.split('\n')[0] ?? '';
}

/** A job's name: what the agent said it was for, else its command — and a
 * server is what it serves on. */
function nameOf(job: Job): string {
  const port = job.ended ? undefined : job.ports[0];
  if (port !== undefined) return `Serving on :${port}`;
  return job.description || commandOf(job);
}

/** How long ago, as the sheet counts it: under a minute is "just now". */
function ago(ms: number): string {
  return ms < 60_000 ? 'just now' : `${compactDuration(ms)} ago`;
}

/** How long a job has run, as the desktop's sidebar counts it. */
function runFor(ms: number): string {
  return ms < 60_000 ? '<1m' : compactDuration(ms);
}

/** How a job ended, in the words its row ends its line with. */
function endedWords(job: Job, now: number): string {
  const ended = job.ended;
  if (!ended) return '';
  const how =
    ended.exit !== undefined
      ? `exit ${ended.exit}`
      : ended.outcome === JobEnded_Outcome.COMPLETED
        ? 'done'
        : ended.outcome === JobEnded_Outcome.FAILED
          ? 'failed'
          : ended.outcome === JobEnded_Outcome.STOPPED
            ? 'stopped'
            : 'ended';
  return `${how} · ${ago(now - Number(ended.atMs))}`;
}

/** The mark a job's row starts with. */
function markOf(job: Job) {
  if (!job.ended) {
    return job.ports.length > 0 ? <Globe size={18} color={t.attention} /> : <Loader size={18} color={t.running} weight={2.2} />;
  }
  switch (job.ended.outcome) {
    case JobEnded_Outcome.FAILED:
      return <Close size={18} color={t.failed} />;
    case JobEnded_Outcome.STOPPED:
      return <Stop size={16} color={t.faint} />;
    default:
      return <Check size={18} color={t.faint} />;
  }
}

export function JobsSheet({
  open,
  onClose,
  agent,
  jobs,
  stop,
}: {
  open: boolean;
  onClose: () => void;
  /** The agent that started them. */
  agent: string;
  /** Running first, then ended, each oldest first — as the desktop sends them. */
  jobs: Job[];
  /** Ends `job`; `null` once it has been signalled, or why not. */
  stop: (job: Job) => Promise<string | null>;
}) {
  const now = useNow();
  const [stopping, setStopping] = useState<ReadonlySet<string>>(new Set());
  const [copied, setCopied] = useState('');
  const [problem, setProblem] = useState('');
  const running = jobs.filter((job) => !job.ended);
  const ended = jobs.filter((job) => job.ended);

  const press = (job: Job) => {
    const key = keyOf(job);
    setProblem('');
    setStopping((was) => new Set(was).add(key));
    // A request that never got an answer — the connection dropped, the Mac
    // went quiet — is a reason too, so the button comes back to be tried
    // again rather than staying on Stopping… for good.
    const asked = stop(job).catch(() => 'Your Mac didn’t answer. Try again once it’s connected.');
    void asked.then((why) => {
      if (!why) return;
      setProblem(why);
      setStopping((was) => {
        const next = new Set(was);
        next.delete(key);
        return next;
      });
    });
  };
  // A row's command, on the clipboard: to run it again, or to look for it.
  const copy = (job: Job) => {
    const key = keyOf(job);
    void Clipboard.setStringAsync(job.command);
    setCopied(key);
    setTimeout(() => setCopied((shown) => (shown === key ? '' : shown)), 1500);
  };

  return (
    <Sheet open={open} onClose={onClose} title="In the background" detail={`Started by ${agentLabel(agent)}, on your Mac`}>
      {running.length > 0 ? (
        <SheetList>
          {running.map((job) => {
            const key = keyOf(job);
            const busy = stopping.has(key);
            return (
              <SheetItem
                key={key}
                label={nameOf(job)}
                mark={markOf(job)}
                detail={copied === key ? 'Command copied' : `${commandOf(job)} · ${runFor(now - Number(job.startedAtMs))}`}
                icon={
                  <Pill
                    title={busy ? 'Stopping…' : 'Stop'}
                    label={`Stop ${nameOf(job)}`}
                    icon={<Stop size={12} color={t.text} />}
                    onPress={() => {
                      if (!busy) press(job);
                    }}
                  />
                }
                onPress={() => copy(job)}
              />
            );
          })}
        </SheetList>
      ) : (
        <SheetNote>Nothing is running in the background now.</SheetNote>
      )}
      {problem ? <SheetNote>{problem}</SheetNote> : null}
      {ended.length > 0 ? (
        <View style={styles.section}>
          <SectionLabel color={t.faint}>Finished</SectionLabel>
          <SheetList>
            {ended.map((job) => {
              const key = keyOf(job);
              return (
                <SheetItem
                  key={key}
                  label={nameOf(job)}
                  mark={markOf(job)}
                  detail={copied === key ? 'Command copied' : endedWords(job, now)}
                  onPress={() => copy(job)}
                />
              );
            })}
          </SheetList>
        </View>
      ) : null}
      {running.some((job) => job.ports.length > 0) ? (
        <SheetNote>A server runs on your Mac’s own localhost, so it opens there, not here.</SheetNote>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  section: { gap: 8 },
});
