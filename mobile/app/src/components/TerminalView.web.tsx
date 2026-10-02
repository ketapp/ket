// A host terminal in a browser: the same xterm.js page the phone's WebView
// shows, in an iframe. The page talks to its host through
// `window.ReactNativeWebView.postMessage`; here that is a shim that hands the
// message to this window instead.

import { forwardRef, useEffect, useImperativeHandle, useRef } from 'react';
import { toBase64Url } from '@ket/remote';

import { page } from '../lib/terminal-page';
import { t } from '../lib/theme';
import type { TerminalHandle } from './TerminalView';

export type { TerminalHandle } from './TerminalView';

interface Props {
  onInput(data: string): void;
  onReady?(): void;
  /** The cells that fit on screen, while fitted. */
  onSize?(cols: number, rows: number): void;
}

const shim =
  '<script>window.ReactNativeWebView = { postMessage: function (m) { parent.postMessage({ ketTerminal: m }, "*"); } };</script>';
const framed = page.replace('<head>', `<head>${shim}`);

export const TerminalView = forwardRef<TerminalHandle, Props>(function TerminalView({ onInput, onReady, onSize }, ref) {
  const frame = useRef<HTMLIFrameElement>(null);
  const call = (message: object) => {
    const target = frame.current?.contentWindow as (Window & { ket?: (m: object) => void }) | null;
    target?.ket?.(message);
  };

  useImperativeHandle(ref, () => ({
    reset: (screen) =>
      call({
        type: 'reset',
        cols: screen.cols,
        rows: screen.rows,
        scrollback: screen.scrollback,
        ansi: toBase64Url(screen.ansi),
      }),
    write: (bytes) => call({ type: 'write', bytes: toBase64Url(bytes) }),
    setInteractive: (on) => call({ type: 'interactive', on }),
    setFit: (on) => call({ type: 'fit', on }),
    focus: () => call({ type: 'focus' }),
  }));

  // Kept in refs so the listener is added once, not on every render.
  const handlers = useRef({ onInput, onReady, onSize });
  handlers.current = { onInput, onReady, onSize };
  useEffect(() => {
    const listen = (event: MessageEvent) => {
      if (event.source !== frame.current?.contentWindow) return;
      const raw = (event.data as { ketTerminal?: string } | null)?.ketTerminal;
      if (typeof raw !== 'string') return;
      const message = JSON.parse(raw) as { type: string; data?: string; url?: string; cols?: number; rows?: number };
      if (message.type === 'size' && message.cols && message.rows) handlers.current.onSize?.(message.cols, message.rows);
      if (message.type === 'input' && message.data) handlers.current.onInput(message.data);
      // Only web addresses: the page found it with a pattern that admits no
      // other scheme, and this checks again rather than trusting the frame.
      if (message.type === 'open' && message.url && /^https?:\/\//.test(message.url)) {
        window.open(message.url, '_blank', 'noopener,noreferrer');
      }
      if (message.type === 'ready') handlers.current.onReady?.();
    };
    window.addEventListener('message', listen);
    return () => window.removeEventListener('message', listen);
  }, []);

  return (
    <iframe
      ref={frame}
      srcDoc={framed}
      title="terminal"
      style={{
        flex: 1,
        width: '100%',
        height: '100%',
        border: 0,
        background: t.card,
      }}
    />
  );
});
