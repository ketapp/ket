// ket's Desk theme, for the phone: the desktop's own tokens
// (crates/ket-core/themes/dark.toml) and the spec in design/STYLE-GUIDE.md.
// The screen is a desk of cards: a near-black ground, cards on a 10px gutter,
// wells set into them, floats above. Two faces, as on the desktop: Geist for
// everything the app says, figures and names included, and Fira Code for
// code and for what a terminal draws. Colour is spent on
// state, never decoration.
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

/** The Keychain class lib/store.ts writes with, which also moves these two
 * entries to it: readable after the first unlock, not only while unlocked. */
const KEYCHAIN: SecureStore.SecureStoreOptions = { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK };

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

/** A colour in OKLab, where equal steps look equal. */
type Lab = [number, number, number];

function toLab(hex: string): Lab {
  const n = parseInt(hex.slice(1), 16);
  const [r, g, b] = [16, 8, 0].map((shift) => {
    const c = ((n >> shift) & 255) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

function fromLab([L, a, b]: Lab): string {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const channels = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return `#${channels
    .map((x) => {
      const c = Math.min(1, Math.max(0, x));
      const v = c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
      return Math.round(v * 255)
        .toString(16)
        .padStart(2, '0');
    })
    .join('')}`;
}

/** How far each grey stands from a theme's surface towards its text. */
const STEP = { card: 0.027, raised: 0.121, active: 0.215, line: 0.289, quiet: 0.638, soft: 0.788 } as const;

/**
 * The phone's greys: one ramp, from a theme's surface to its text, in steps
 * that look even. A theme says three colours — its backdrop, its surface and
 * its text — and every grey the phone draws with is one of eight:
 *
 * - the ground (`backdrop`), as the theme has it;
 * - the surface (`card`), a touch further from the ground than the theme's;
 * - raised (`sunken`, `elevated`, `hover`): a well, a chip, a float;
 * - active (`selection`): what is chosen, or under a finger;
 * - a line (`border`), and the hairline round a card, which is the raised
 *   grey (`rule`): an edge is one step above the fill it outlines;
 * - three inks: quiet (`faint`), soft (`dim`) and the text.
 *
 * A theme's own sunken, elevated, hover, selection, border, rule and dim are
 * not used: on Desk three of them were one grey to the eye, and no two
 * themes spaced them alike. The quiet ink is moved towards the text until it
 * reads at 4.5:1 on a raised fill, the darkest thing words stand on.
 */
export function ramp(backdrop: string, surface: string, text: string) {
  const from = toLab(surface);
  const to = toLab(text);
  const at = (k: number) => fromLab([0, 1, 2].map((i) => from[i] + (to[i] - from[i]) * k) as Lab);
  // Towards the text is away from the ground on a dark theme, where the
  // card gains by it. On a light one the ground is that way too.
  const lifts = (toLab(backdrop)[0] - from[0]) * (to[0] - from[0]) <= 0;
  const raised = at(STEP.raised);
  let quiet: number = STEP.quiet;
  while (quiet < 0.9 && contrast(at(quiet), raised) < 4.5) quiet += 0.02;
  return {
    backdrop,
    card: lifts ? at(STEP.card) : surface,
    sunken: raised,
    elevated: raised,
    hover: raised,
    selection: at(STEP.active),
    border: at(STEP.line),
    rule: raised,
    faint: at(quiet),
    dim: at(Math.min(0.95, Math.max(STEP.soft, quiet + 0.15))),
    text,
  };
}

/** The colours: the desktop's theme when it has said one, with its greys
 * redrawn on the phone's ramp. */
export const t: Tokens = (() => {
  const said = { ...DESK, ...stored };
  return { ...said, ...ramp(said.backdrop, said.card, said.text), brand: DESK.brand, onBrand: DESK.onBrand };
})();

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
    SecureStore.setItem(PALETTE_KEY, next, KEYCHAIN);
    kept = next;
    const last = Number(SecureStore.getItem(RELOADED_KEY) ?? 0);
    if (Date.now() - last < RELOAD_GAP_MS) return;
    SecureStore.setItem(RELOADED_KEY, String(Date.now()), KEYCHAIN);
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

/** `over` at some opacity laid on `under`, as one solid colour: the tint
 * for something drawn over other things, which must not show through. */
export function blend(under: string, over: string, alpha: number): string {
  const a = parseInt(under.slice(1), 16);
  const b = parseInt(over.slice(1), 16);
  const channel = (shift: number) =>
    Math.round(((a >> shift) & 255) * (1 - alpha) + ((b >> shift) & 255) * alpha)
      .toString(16)
      .padStart(2, '0');
  return `#${channel(16)}${channel(8)}${channel(0)}`;
}

export const font = {
  chrome: 'Geist_400Regular',
  chromeMedium: 'Geist_500Medium',
  chromeSemibold: 'Geist_600SemiBold',
  prose: 'Geist_400Regular',
  proseMedium: 'Geist_500Medium',
  proseSemibold: 'Geist_600SemiBold',
  /** A display heading, and nothing smaller. */
  proseBlack: 'Geist_900Black',
  code: 'FiraCode_400Regular',
  codeSemibold: 'FiraCode_600SemiBold',
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
  row: 58,
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
  // OpenAI's mark is grey: the ramp's quiet ink, not a grey of its own.
  codex: t.faint,
  // OpenCode's mark is one colour, and its brand allows no other.
  opencode: t.text,
  gemini: '#8e7bf5',
  // xAI's mark has no colour of its own; the shape tells it apart.
  grok: t.text,
};

/** A Claude session's `/color` (protocol 1.31) as the desktop paints it:
 * Tailwind's 500 — see `ket_core::agent_color`. */
const SESSION_COLORS: Record<string, string> = {
  red: '#ef4444',
  blue: '#3b82f6',
  green: '#22c55e',
  yellow: '#eab308',
  purple: '#a855f7',
  orange: '#f97316',
  pink: '#ec4899',
  cyan: '#06b6d4',
};

function luminance(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  const [r, g, b] = [16, 8, 0].map((shift) => {
    const c = ((n >> shift) & 255) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** How a session's `/color` is worn, as the desktop wears it: its mark's
 * tile, the mark on it in whichever of the card and the text stands off it
 * more, and its name moved towards the text until it reads at 4.5:1 on a
 * card. `undefined` for none, or a colour this build doesn't know. */
export function sessionColor(name: string | undefined): { tile: string; ink: string; label: string } | undefined {
  const tile = name ? SESSION_COLORS[name] : undefined;
  if (!tile) return undefined;
  const ink = contrast(t.card, tile) >= contrast(t.text, tile) ? t.card : t.text;
  let label = t.text;
  for (let step = 0; step <= 20; step++) {
    const candidate = mix(tile, t.text, step / 20);
    if (contrast(candidate, t.card) >= 4.5) {
      label = candidate;
      break;
    }
  }
  return { tile, ink, label };
}

/** The program behind an agent's name: `opencode:ollama/<model>` is OpenCode,
 * `ollama:<model>` is Ollama. What a mark or a colour is looked up by. */
export function agentHarness(agent: string): string {
  return agent.split(':', 1)[0].toLowerCase();
}

/** An agent's name as people write it — "OpenCode", not "Opencode" — as the
 * desktop's labels have it. An agent ket has no spelling for is capitalised. */
export function agentLabel(agent: string): string {
  // OpenCode driving a local model: the model, then the harness.
  const local = /^opencode:ollama\/(.+)$/i.exec(agent.trim());
  if (local) return `${local[1]} · OpenCode`;
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

/** A short recipient name for the phone chat's message field. */
export function messagePlaceholder(agent: string | undefined): string {
  const name = agent?.trim();
  if (!name) return 'Message the agent';

  const localModel = /^(?:opencode:ollama\/|ollama:)(.+)$/i.exec(name)?.[1];
  const model = localModel?.split('/').pop()?.split(':', 1)[0];
  if (model && /^qwen(?:\d|[-_.]|$)/i.test(model)) return 'Message qwen';
  const label = model || agentLabel(name);
  const characters = Array.from(label);
  return `Message ${characters.length > 16 ? `${characters.slice(0, 15).join('')}…` : label}`;
}
