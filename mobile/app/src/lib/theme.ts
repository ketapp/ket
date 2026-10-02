// ket's Desk theme, for the phone: the desktop's own tokens
// (crates/ket-core/themes/dark.toml) and the spec in design/STYLE-GUIDE.md.
// The screen is a desk of cards: a near-black ground, cards on a 10px gutter,
// wells set into them, floats above. Geist for words, Geist Mono for anything
// you would quote, IBM Plex Mono for what a terminal draws. Colour is spent
// on state, never decoration.
//
// The colours follow the desktop. Whatever theme ket is drawn in there, the
// host sends as a palette (`HostSnapshot.palette`), and the phone keeps the
// last one it was sent. It is read here, synchronously, before any screen
// builds its styles — styles are made once, at load — and a palette that
// differs from the one the app was drawn with reloads the app to draw in it.
// See `adoptPalette`. The phone has no theme setting of its own.

import type { Palette } from '@ket/remote';
import { reloadAppAsync } from 'expo';
import * as SecureStore from 'expo-secure-store';

/** Desk, as the phone drew it before any desktop said otherwise. */
const DESK = {
  /** The desk: the ground between cards. */
  backdrop: '#050607',
  /** A card: every region of the screen. */
  card: '#0d0f12',
  /** A well set into a card: fields, segmented tracks, key rows. */
  sunken: '#14171b',
  /** A float: menus, sheets, a pressed key. */
  elevated: '#181b20',
  hover: '#171a1f',
  selection: '#22262c',
  border: '#262a31',
  rule: '#1a1d22',
  accent: '#eceef1',
  /** The primary fill under a finger. */
  accentPressed: '#ffffff',
  onAccent: '#0d0f12',
  text: '#e7e9ec',
  dim: '#8b929b',
  /** Placeholders and glyphs at rest. */
  faint: '#6b727c',
  running: '#4cd08a',
  /** Waiting on a person: blue, so it never reads as the marker. */
  attention: '#7aa7ff',
  failed: '#ef6a74',
  merging: '#b8a7e0',
  added: '#4cd08a',
  removed: '#ef6a74',
  modified: '#f5c451',
  /** The selected row's rail. Nothing else. */
  marker: '#f59e0b',
  /** The mark in ket's app icon. */
  brand: '#7ee08a',
  /** Ink on a brand fill. */
  onBrand: '#06170b',
  /** A plan quota past seven tenths: worth a glance. */
  quotaWarm: '#ffe45c',
  /** Past eight tenths: worth planning around. */
  quotaHot: '#ff9433',
  /** Past nine tenths: about to stop. */
  quotaCritical: '#ff5a4a',
};

/** The colours a screen draws with. */
export type Tokens = typeof DESK;

/** Where the desktop's last palette is kept. */
const PALETTE_KEY = 'ket.palette';

/** What a stored palette may set: every colour token but ket's own brand. */
type Stored = { light: boolean } & Partial<Omit<Tokens, 'brand' | 'onBrand'>>;

/** The palette as it was stored, word for word — what a new one is compared
 * with, so the same palette arriving again changes nothing. */
const saved = (() => {
  try {
    return SecureStore.getItem(PALETTE_KEY);
  } catch {
    return null;
  }
})();

/** The stored palette, keeping only well-formed colours. */
const stored: Stored | null = (() => {
  if (!saved) return null;
  try {
    const parsed = JSON.parse(saved) as Record<string, unknown>;
    const out: Record<string, unknown> = { light: parsed.light === true };
    for (const key of Object.keys(DESK)) {
      const value = parsed[key];
      if (typeof value === 'string' && /^#[0-9a-f]{6}$/i.test(value)) out[key] = value;
    }
    return out as Stored;
  } catch {
    return null;
  }
})();

/** The colours, in the desktop's theme when it has said one. */
export const t: Tokens = { ...DESK, ...stored, brand: DESK.brand, onBrand: DESK.onBrand };

/** Whether the desktop's theme is a light one: dark ink, a dark status bar. */
export const light = stored?.light ?? false;

/** Two colours mixed, `amount` of the way from `a` to `b`. */
function mix(a: string, b: string, amount: number): string {
  const ca = parseInt(a.slice(1), 16);
  const cb = parseInt(b.slice(1), 16);
  const channel = (shift: number) => {
    const x = (ca >> shift) & 255;
    const y = (cb >> shift) & 255;
    return Math.round(x + (y - x) * amount);
  };
  return `#${[16, 8, 0].map((shift) => channel(shift).toString(16).padStart(2, '0')).join('')}`;
}

/** A desktop palette in the phone's tokens. The two the desktop has no
 * token for are made from ones it has: placeholders a third of the way from
 * the dim line into the card, and a pressed primary lifted towards white. */
function fromPalette(palette: Palette): Stored {
  return {
    light: palette.light,
    backdrop: palette.backdrop,
    card: palette.surface,
    sunken: palette.sunken,
    elevated: palette.elevated,
    hover: palette.hover,
    selection: palette.selection,
    border: palette.border,
    rule: palette.rule,
    accent: palette.accent,
    accentPressed: mix(palette.accent, '#ffffff', 0.35),
    onAccent: palette.onAccent,
    text: palette.text,
    dim: palette.dim,
    faint: mix(palette.dim, palette.surface, 0.3),
    running: palette.running,
    attention: palette.attention,
    failed: palette.failed,
    merging: palette.merging,
    added: palette.added,
    removed: palette.removed,
    modified: palette.modified,
    marker: palette.marker,
    quotaWarm: palette.quotaWarm,
    quotaHot: palette.quotaHot,
    quotaCritical: palette.quotaCritical,
  };
}

/** When the app last reloaded to take a palette, in Unix milliseconds. */
const RELOADED_KEY = 'ket.palette.reloadedAt';

/** How long after one palette reload another is held back. Within it a new
 * palette is kept and drawn the next time the app starts. */
const RELOAD_GAP_MS = 60_000;

/** The palette kept since the app started, so the same one arriving with
 * every snapshot is acted on once. */
let kept: string | null = null;

/** Takes the desktop's palette: kept, and the app reloaded to draw in it —
 * unless it is the one the app is already drawn in, so it settles after one
 * reload. A palette that cannot be kept is not acted on, so a failing store
 * cannot reload the app over and over.
 *
 * At most one reload a minute, whatever arrives. A palette that never
 * settled — two desktops in different themes, answering in a different
 * order after each start — restarted the app every few seconds, which read
 * as the app connecting and then quitting. Inside the minute the palette is
 * kept, and drawn on the next start. */
export function adoptPalette(palette: Palette): void {
  const next = JSON.stringify(fromPalette(palette));
  if (next === saved || next === kept) return;
  try {
    SecureStore.setItem(PALETTE_KEY, next);
    kept = next;
    const last = Number(SecureStore.getItem(RELOADED_KEY) ?? 0);
    if (Date.now() - last < RELOAD_GAP_MS) return;
    SecureStore.setItem(RELOADED_KEY, String(Date.now()));
  } catch {
    return;
  }
  void reloadAppAsync('the desktop changed its theme');
}

/** A colour at some opacity, for tints: `tint(t.attention, 0.08)`. */
export function tint(hex: string, alpha: number): string {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

export const font = {
  chrome: 'GeistMono_400Regular',
  chromeMedium: 'GeistMono_500Medium',
  chromeSemibold: 'GeistMono_600SemiBold',
  prose: 'Geist_400Regular',
  proseMedium: 'Geist_500Medium',
  proseSemibold: 'Geist_600SemiBold',
  /** A display heading, and nothing smaller. */
  proseBlack: 'Geist_900Black',
  code: 'IBMPlexMono_400Regular',
  codeSemibold: 'IBMPlexMono_600SemiBold',
} as const;

export const size = {
  caption: 11,
  detail: 12.5,
  label: 13,
  body: 14,
  name: 14,
  value: 15,
  title: 15,
  /** A worktree row. */
  row: 56,
  /** A plain list row: a setting, a device. */
  rowSm: 48,
  control: 44,
  /** Chips, segments, keys. */
  radiusSm: 5,
  /** Buttons, fields, tracks, wells. */
  radius: 7,
  /** List rows. */
  radiusRow: 8,
  /** Floats. */
  radiusLg: 10,
  /** Cards. */
  radiusCard: 12,
  /** Between cards, and from a card to the screen's edge. */
  gutter: 10,
  /** Inside a card. */
  pad: 12,
  bar: 52,
} as const;

/** ket's project colours; the first is the one a project has by default. */
export const PROJECT_COLORS = [
  '#737373',
  '#ef4444',
  '#f97316',
  '#eab308',
  '#22c55e',
  '#14b8a6',
  '#8b5cf6',
  '#ec4899',
] as const;

/** Agent brand colours, as ket's sidebar draws their marks. */
export const AGENT_COLORS: Record<string, string> = {
  claude: '#d97757',
  codex: '#aab0b8',
  opencode: '#e8b339',
  gemini: '#8e7bf5',
  // xAI's mark has no colour of its own; the shape tells it apart.
  grok: t.text,
};

/** An agent's name as people write it — "OpenCode", not "Opencode" — as the
 * desktop's labels have it. An agent ket has no spelling for is capitalised. */
export function agentLabel(agent: string): string {
  const known: Record<string, string> = {
    claude: 'Claude',
    codex: 'Codex',
    opencode: 'OpenCode',
    gemini: 'Gemini',
    grok: 'Grok',
  };
  const name = agent.trim();
  return known[name.toLowerCase()] ?? name.charAt(0).toUpperCase() + name.slice(1);
}
