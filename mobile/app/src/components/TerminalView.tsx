// A host terminal on the phone: xterm.js in a WebView, fed the checkpoint and
// the output after it.
//
// The phone never resizes the host's terminal on its own: it
// draws the host's columns at whatever font size fits the screen, down to a
// floor, and pans sideways past that. Typing goes nowhere unless the phone has
// taken control; the screen calls `setInteractive` when it has.

import { forwardRef, useImperativeHandle, useMemo, useRef } from 'react';
import { Linking, StyleSheet } from 'react-native';
import { WebView, type WebViewMessageEvent } from 'react-native-webview';
import { toBase64Url } from '@ket/remote';

import { page } from '../lib/terminal-page';
import { t } from '../lib/theme';

export interface TerminalHandle {
  reset(screen: { cols: number; rows: number; scrollback: number; ansi: Uint8Array }): void;
  write(bytes: Uint8Array): void;
  setInteractive(on: boolean): void;
  /** Fitted to the phone: readable type, and the size in cells reported
   * through `onSize` so the host's terminal can be resized to it. */
  setFit(on: boolean): void;
  focus(): void;
}

interface Props {
  onInput(data: string): void;
  onReady?(): void;
  /** The cells that fit on screen, while fitted. */
  onSize?(cols: number, rows: number): void;
}

export const TerminalView = forwardRef<TerminalHandle, Props>(function TerminalView(
  { onInput, onReady, onSize },
  ref,
) {
  const webview = useRef<WebView>(null);
  const call = (message: object) =>
    webview.current?.injectJavaScript(`window.ket(${JSON.stringify(message)}); true;`);

  // Output is handed to the page once a frame, not per chunk: an agent
  // streaming sends many small chunks, and each injection is a trip across
  // to the WebView that competes with the finger scrolling it.
  const queued = useRef<Uint8Array[]>([]);
  const scheduled = useRef(false);
  const flush = () => {
    scheduled.current = false;
    const parts = queued.current;
    queued.current = [];
    if (parts.length === 0) return;
    const bytes = parts.length === 1 ? parts[0] : join(parts);
    call({ type: 'write', bytes: toBase64Url(bytes) });
  };

  useImperativeHandle(ref, () => ({
    reset: (screen) => {
      // Output queued before a checkpoint is already in it.
      queued.current = [];
      call({
        type: 'reset',
        cols: screen.cols,
        rows: screen.rows,
        scrollback: screen.scrollback,
        ansi: toBase64Url(screen.ansi),
      });
    },
    write: (bytes) => {
      queued.current.push(bytes);
      if (!scheduled.current) {
        scheduled.current = true;
        requestAnimationFrame(flush);
      }
    },
    setInteractive: (on) => call({ type: 'interactive', on }),
    setFit: (on) => call({ type: 'fit', on }),
    focus: () => call({ type: 'focus' }),
  }));

  const source = useMemo(() => ({ html: page }), []);
  const onMessage = (event: WebViewMessageEvent) => {
    const message = JSON.parse(event.nativeEvent.data) as { type: string; data?: string; url?: string; cols?: number; rows?: number };
    if (message.type === 'size' && message.cols && message.rows) onSize?.(message.cols, message.rows);
    if (message.type === 'input' && message.data) onInput(message.data);
    // Only web addresses, checked here rather than trusted from the page.
    if (message.type === 'open' && message.url && /^https?:\/\//.test(message.url)) void Linking.openURL(message.url);
    if (message.type === 'ready') onReady?.();
  };

  return (
    <WebView
      ref={webview}
      style={styles.webview}
      originWhitelist={['*']}
      source={source}
      onMessage={onMessage}
      keyboardDisplayRequiresUserAction={false}
      hideKeyboardAccessoryView
      scrollEnabled
      showsVerticalScrollIndicator={false}
      showsHorizontalScrollIndicator={false}
      bounces={false}
      automaticallyAdjustContentInsets={false}
    />
  );
});

function join(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((total, part) => total + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

const styles = StyleSheet.create({
  webview: { flex: 1, backgroundColor: t.card },
});
