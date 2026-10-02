// The handful of glyphs the app draws: ket's own mark, the agents' marks as
// the desktop sidebar shows them, and plain stroke icons.

import { BacklogPriority } from '@ket/remote';
import Svg, { Circle, G, Path, Polygon, Rect } from 'react-native-svg';

import { AGENT_COLORS, t } from '../lib/theme';

type IconProps = { size?: number; color?: string; weight?: number };

/** ket's mark: the bar and chevron from the app icon. */
export function BrandMark({ size = 18 }: { size?: number }) {
  return (
    <Svg width={size * 0.88} height={size} viewBox="290 260 445 505">
      <Rect x={302} y={272} width={100} height={480} fill={t.brand} />
      <Polygon points="438,272 545,272 722,512 545,752 438,752 540,512" fill={t.brand} />
    </Svg>
  );
}

/** An agent's mark in its own colour, or nothing for an agent ket has no mark for. */
export function AgentMark({ agent, size = 14 }: { agent: string; size?: number }) {
  const name = agent.toLowerCase();
  const color = AGENT_COLORS[name] ?? t.dim;
  if (name === 'codex') {
    return (
      <Svg width={size} height={size} viewBox="0 0 24 24">
        <Path d="M12 2.5 20.5 7.5v9L12 21.5 3.5 16.5v-9Z" fill="none" stroke={color} strokeWidth={2.2} strokeLinejoin="round" />
      </Svg>
    );
  }
  if (name === 'grok') {
    return (
      <Svg width={size} height={size} viewBox="0 -0.5 34 34">
        <Path
          d="M13.2371 21.0407L24.3186 12.8506C24.8619 12.4491 25.6384 12.6057 25.8973 13.2294C27.2597 16.5185 26.651 20.4712 23.9403 23.1851C21.2297 25.8989 17.4581 26.4941 14.0108 25.1386L10.2449 26.8843C15.6463 30.5806 22.2053 29.6665 26.304 25.5601C29.5551 22.3051 30.562 17.8683 29.6205 13.8673L29.629 13.8758C28.2637 7.99809 29.9647 5.64871 33.449 0.844576C33.5314 0.730667 33.6139 0.616757 33.6964 0.5L29.1113 5.09055V5.07631L13.2343 21.0436"
          fill={color}
        />
        <Path
          d="M10.9503 23.0313C7.07343 19.3235 7.74185 13.5853 11.0498 10.2763C13.4959 7.82722 17.5036 6.82767 21.0021 8.2971L24.7595 6.55998C24.0826 6.07017 23.215 5.54334 22.2195 5.17313C17.7198 3.31926 12.3326 4.24192 8.67479 7.90126C5.15635 11.4239 4.0499 16.8403 5.94992 21.4622C7.36924 24.9165 5.04257 27.3598 2.69884 29.826C1.86829 30.7002 1.0349 31.5745 0.36364 32.5L10.9474 23.0341"
          fill={color}
        />
      </Svg>
    );
  }
  if (name === 'opencode') {
    return (
      <Svg width={size} height={size} viewBox="0 0 24 24">
        <Path d="m4.5 6.5 6 5.5-6 5.5M13 17.5h6.5" fill="none" stroke={color} strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round" />
      </Svg>
    );
  }
  if (name === 'claude') {
    return (
      <Svg width={size} height={size} viewBox="0 0 24 24">
        <G stroke={color} strokeWidth={2.8} strokeLinecap="round">
          <Path d="M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6 5.6 18.4" />
        </G>
      </Svg>
    );
  }
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24">
      <Circle cx={12} cy={12} r={5} fill={color} />
    </Svg>
  );
}

function Stroke({ size = 18, color = t.dim, weight = 1.8, children }: IconProps & { children: React.ReactNode }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={weight} strokeLinecap="round" strokeLinejoin="round">
      {children}
    </Svg>
  );
}

export const Plus = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 5v14M5 12h14" />
  </Stroke>
);

export const Back = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="m15 18-6-6 6-6" />
  </Stroke>
);

/** Leaves the app: a link that opens in the browser. */
export const External = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M7 7h10v10" />
    <Path d="M7 17 17 7" />
  </Stroke>
);

export const Chevron = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="m9 18 6-6-6-6" />
  </Stroke>
);

export const Desktop = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={3} y={4} width={18} height={12} rx={1.5} />
    <Path d="M12 16v4M8 20h8" />
  </Stroke>
);

export const Sliders = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M4 7.5h8.5M17.5 7.5H20M4 16.5h2.5M11.5 16.5H20" />
    <Circle cx={15} cy={7.5} r={2.5} />
    <Circle cx={9} cy={16.5} r={2.5} />
  </Stroke>
);

export const Search = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={11} cy={11} r={6} />
    <Path d="m20 20-4.5-4.5" />
  </Stroke>
);

export const TerminalGlyph = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="m5 17 5-5-5-5" />
    <Path d="M12 19h7" />
  </Stroke>
);

export const Branch = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={6} cy={5} r={2} />
    <Circle cx={6} cy={19} r={2} />
    <Circle cx={18} cy={7} r={2} />
    <Path d="M6 7v10M18 9c0 5-7 4-11 8" />
  </Stroke>
);

export const Clipboard = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={8} y={3} width={8} height={4} rx={1} />
    <Path d="M16 5h2a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h2" />
  </Stroke>
);

export const Send = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 19V5M5 12l7-7 7 7" />
  </Stroke>
);

/** Worktrees: a trunk with a branch off it. */
export const Tree = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={6} cy={5.5} r={2.25} />
    <Circle cx={6} cy={18.5} r={2.25} />
    <Circle cx={18} cy={8} r={2.25} />
    <Path d="M6 7.75v8.5M18 10.25c0 4.6-4.2 5.4-9.9 7.1" />
  </Stroke>
);

/** Usage: a gauge's open arc and its needle. */
export const Gauge = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M4.2 17.5a8.5 8.5 0 1 1 15.6 0" />
    <Path d="m12 14 3.6-4.6" />
    <Circle cx={12} cy={14} r={1.3} />
  </Stroke>
);

/** A project's backlog: a checked box beside lines, as on the desktop. */
export const ListTodo = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M13 5h8M13 12h8M13 19h8" />
    <Path d="m3 17 2 2 4-4" />
    <Rect x={3} y={4} width={6} height={6} rx={1} />
  </Stroke>
);

/** A backlog priority's colour: the quota meter's ramp, since both say how
 * pressing something is, with Low left dim so it recedes. Unspecified — a
 * desktop that keeps none — is Medium's. */
export function priorityColor(priority: BacklogPriority): string {
  switch (priority) {
    case BacklogPriority.LOW:
      return t.dim;
    case BacklogPriority.HIGH:
      return t.quotaHot;
    case BacklogPriority.URGENT:
      return t.quotaCritical;
    default:
      return t.quotaWarm;
  }
}

/** A backlog note's priority, as the desktop draws it: one, two or three of
 * three bars lit — or for Urgent a filled square with an exclamation mark,
 * so it stands out of a list of bars. Faded for a done note. */
export function PriorityMark({ priority, size = 14, faded }: { priority: BacklogPriority; size?: number; faded?: boolean }) {
  const color = priorityColor(priority);
  if (priority === BacklogPriority.URGENT) {
    return (
      <Svg width={size} height={size} viewBox="0 0 24 24" opacity={faded ? 0.5 : 1}>
        <Path
          fill={color}
          fillRule="evenodd"
          d="M7 3h10a4 4 0 0 1 4 4v10a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V7a4 4 0 0 1 4-4zM12 6.5a1.25 1.25 0 0 1 1.25 1.25v5a1.25 1.25 0 0 1-2.5 0v-5A1.25 1.25 0 0 1 12 6.5zM12 15.35a1.4 1.4 0 1 1 0 2.8a1.4 1.4 0 1 1 0-2.8z"
        />
      </Svg>
    );
  }
  const lit = priority === BacklogPriority.LOW ? 1 : priority === BacklogPriority.HIGH ? 3 : 2;
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" opacity={faded ? 0.5 : 1}>
      <Rect x={4} y={14} width={4} height={6} rx={1.5} fill={color} />
      <Rect x={10} y={9} width={4} height={11} rx={1.5} fill={color} opacity={lit >= 2 ? 1 : 0.38} />
      <Rect x={16} y={4} width={4} height={16} rx={1.5} fill={color} opacity={lit >= 3 ? 1 : 0.38} />
    </Svg>
  );
}

/** What waits on you: a tray. */
export const Inbox = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M3.5 13.5 6.2 5.6A2 2 0 0 1 8.1 4.25h7.8a2 2 0 0 1 1.9 1.35l2.7 7.9V18a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />
    <Path d="M3.5 13.5h4.6l1.4 2.5h5l1.4-2.5h4.6" />
  </Stroke>
);

/** A camera: pointing the phone at a code. */
export const Camera = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M3 8.5A2 2 0 0 1 5 6.5h2.2l1.4-2h6.8l1.4 2H19a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    <Circle cx={12} cy={13} r={3.5} />
  </Stroke>
);

/** A raised hand: an agent waiting on a person. */
export const Hand = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 3v7M8.5 5v6M15.5 5v6" />
    <Path d="M19 8v6a7 7 0 0 1-14 0v-2" />
  </Stroke>
);

/** A keyboard: the row of keys a phone's own keyboard lacks. */
export const Keyboard = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={2.5} y={6} width={19} height={12} rx={2} />
    <Path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10" />
  </Stroke>
);

/** Full screen: corners pulled out. */
export const Expand = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
  </Stroke>
);

/** Out of full screen: corners pushed in. */
export const Collapse = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5" />
  </Stroke>
);

/** Face ID and face unlock: a face in a scanner's corners. */
export const FaceScan = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M3 7V5a2 2 0 0 1 2-2h2M17 3h2a2 2 0 0 1 2 2v2M21 17v2a2 2 0 0 1-2 2h-2M7 21H5a2 2 0 0 1-2-2v-2" />
    <Path d="M8 14s1.5 2 4 2 4-2 4-2M9 9h.01M15 9h.01" />
  </Stroke>
);

/** Touch ID and fingerprint unlock. */
export const Fingerprint = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 10a2 2 0 0 0-2 2c0 1.02-.1 2.51-.26 4" />
    <Path d="M14 13.12c0 2.38 0 6.38-1 8.88" />
    <Path d="M17.29 21.02c.12-.6.43-2.3.5-3.02" />
    <Path d="M2 12a10 10 0 0 1 18-6" />
    <Path d="M2 16h.01" />
    <Path d="M21.8 16c.2-2 .131-5.354 0-6" />
    <Path d="M5 19.5C5.5 18 6 15 6 12a6 6 0 0 1 .34-2" />
    <Path d="M8.65 22c.21-.66.45-1.32.57-2" />
    <Path d="M9 6.8a6 6 0 0 1 9 5.2v2" />
  </Stroke>
);

/** Taking back a digit. */
export const Erase = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M10 5h10a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H10l-7-7z" />
    <Path d="m17 9-6 6M11 9l6 6" />
  </Stroke>
);

/** A saved snippet. */
export const Bookmark = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M7 4h10v16l-5-3.5L7 20z" />
  </Stroke>
);

/** Opens a dropdown. */
export const ChevronDown = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="m6 9 6 6 6-6" />
  </Stroke>
);

/** Dismiss. */
export const Close = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M18 6 6 18M6 6l12 12" />
  </Stroke>
);

/** Picked: a tick. */
export const Check = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="m5 12.5 4.5 4.5L19 7.5" />
  </Stroke>
);

/** Stop what is running: a filled square. */
export const Stop = ({ size = 18, color = t.dim }: IconProps) => (
  <Svg width={size} height={size} viewBox="0 0 24 24">
    <Rect x={6} y={6} width={12} height={12} rx={2.5} fill={color} />
  </Svg>
);
