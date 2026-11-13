// Errors the app did not handle, kept on this phone for a person to send.
//
// ket's privacy policy promises no crash reports reach us, so nothing here
// leaves the phone on its own: the last few errors are written to a file in
// the app's own storage, and Settings › About › Error reports shares them
// through the share sheet when someone chooses to. Two ways in: the root
// layout's error boundary, for a screen that failed to draw, and the global
// handler, for an error thrown anywhere else. A fatal one still closes the
// app, as React Native does without this; its report is waiting on the next
// open. Written synchronously for that reason: the app may be gone a moment
// after.

import { File, Paths } from 'expo-file-system';
import { useCallback, useState } from 'react';
import { Platform } from 'react-native';
import { build, version } from './version';

/** One error, as kept and as shared. */
export interface ErrorReport {
  /** When, as an ISO date. */
  at: string;
  version: string;
  build: string;
  /** Whether it closed the app or took down a screen. */
  fatal: boolean;
  /** Where it was caught: `screen` for the error boundary, `global` otherwise. */
  where: 'screen' | 'global';
  message: string;
  stack: string;
}

/** How many are kept: enough to see a pattern, few enough to read. */
const KEEP = 5;
/** A stack cut to this many characters: the top of it is what says where. */
const STACK = 4000;
const NAME = 'error-reports.json';
const WEB_KEY = 'ket.errorReports';

function file(): File {
  return new File(Paths.document, NAME);
}

/** The kept reports, newest first. Empty when there are none or the file
 * cannot be read. */
export function readReports(): ErrorReport[] {
  try {
    const text = Platform.OS === 'web' ? (globalThis.localStorage?.getItem(WEB_KEY) ?? '') : file().exists ? file().textSync() : '';
    const parsed: unknown = text ? JSON.parse(text) : [];
    return Array.isArray(parsed) ? (parsed as ErrorReport[]) : [];
  } catch {
    return [];
  }
}

function writeReports(reports: ErrorReport[]): void {
  const text = JSON.stringify(reports);
  if (Platform.OS === 'web') {
    globalThis.localStorage?.setItem(WEB_KEY, text);
    return;
  }
  const target = file();
  if (!target.exists) target.create();
  target.writeSync(text);
}

/** Keeps `error`. Never throws: a failure to record must not become a second
 * error inside the handler that is recording the first. */
export function recordError(error: unknown, how: { fatal: boolean; where: ErrorReport['where'] }): void {
  try {
    const shaped = error instanceof Error ? error : new Error(String(error));
    const report: ErrorReport = {
      at: new Date().toISOString(),
      version,
      build,
      fatal: how.fatal,
      where: how.where,
      message: shaped.message || shaped.name || 'Unknown error',
      stack: (shaped.stack ?? '').slice(0, STACK),
    };
    writeReports([report, ...readReports()].slice(0, KEEP));
  } catch {
    // Nothing to do: the error itself still goes where it would have gone.
  }
}

/** Removes every kept report. */
export function clearReports(): void {
  try {
    if (Platform.OS === 'web') globalThis.localStorage?.removeItem(WEB_KEY);
    else if (file().exists) file().delete();
  } catch {
    // A file that cannot be deleted leaves the reports listed, which is honest.
  }
}

/** The reports as plain text, for the share sheet: what someone pastes into an
 * email or an issue. Says what the phone is, not who has it. */
export function reportText(reports: ErrorReport[]): string {
  const head = `ket ${version} (${build}) on ${Platform.OS} ${String(Platform.Version)}`;
  const body = reports.map(
    (report) =>
      `${report.at} · ${report.fatal ? 'closed the app' : 'recovered'} · ${report.where} · ket ${report.version} (${report.build})\n` +
      `${report.message}\n${report.stack}`,
  );
  return [head, ...body].join('\n\n');
}

let installed = false;

/** Records every error that reaches React Native's global handler, then hands
 * it on, so the app behaves as it did without this. Once per process. */
export function installErrorCapture(): void {
  if (installed) return;
  installed = true;
  const utils = (globalThis as { ErrorUtils?: ErrorUtilsShape }).ErrorUtils;
  if (!utils) return;
  const previous = utils.getGlobalHandler();
  utils.setGlobalHandler((error, isFatal) => {
    recordError(error, { fatal: Boolean(isFatal), where: 'global' });
    previous(error, isFatal);
  });
}

interface ErrorUtilsShape {
  getGlobalHandler(): (error: unknown, isFatal?: boolean) => void;
  setGlobalHandler(handler: (error: unknown, isFatal?: boolean) => void): void;
}

/** The kept reports for a screen that shows them, re-read when asked. */
export function useErrorReports(): { reports: ErrorReport[]; refresh: () => void; clear: () => void } {
  const [reports, setReports] = useState(readReports);
  const refresh = useCallback(() => setReports(readReports()), []);
  const clear = useCallback(() => {
    clearReports();
    setReports([]);
  }, []);
  return { reports, refresh, clear };
}
