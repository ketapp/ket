// The handful of glyphs the app draws: ket's own mark, the agents' marks as
// the desktop sidebar shows them, and plain stroke icons.

import { BacklogPriority } from '@ket/remote';
import { StyleSheet, Text, View } from 'react-native';
import Svg, { Circle, Path, Polygon, Rect } from 'react-native-svg';

import type { AddressKind } from '../lib/address-kind';
import { AGENT_COLORS, agentHarness, font, sessionColor, t } from '../lib/theme';

type IconProps = { size?: number; color?: string; weight?: number; fill?: string };

/** ket's mark: the bar and chevron from the app icon. `color` stands in for
 * the brand green where the mark also reports a state. */
export function BrandMark({ size = 18, color = t.brand }: { size?: number; color?: string }) {
  return (
    <Svg width={size * 0.88} height={size} viewBox="290 260 445 505">
      <Rect x={302} y={272} width={100} height={480} fill={color} />
      <Polygon points="438,272 545,272 722,512 545,752 438,752 540,512" fill={color} />
    </Svg>
  );
}

const OPENAI_BLOSSOM =
  'M252.794 108.802C289.191 99.0484 326.265 110.305 351.148 135.135C385.113 126.072 422.85 134.862 449.492 161.505C476.136 188.149 484.925 225.888 475.862 259.85V259.854C500.696 284.735 511.95 321.81 502.198 358.207C492.447 394.602 464.161 421.084 430.215 430.217C421.083 464.162 394.603 492.448 358.206 502.199C321.812 511.951 284.734 500.693 259.852 475.864C225.887 484.927 188.15 476.137 161.507 449.495C134.864 422.851 126.073 385.111 135.136 351.149C110.304 326.266 99.0496 289.192 108.801 252.795C118.552 216.4 146.84 189.918 180.784 180.785C189.917 146.841 216.396 118.553 252.794 108.802ZM374.292 407.145C374.292 411.271 372.092 415.086 368.517 417.148L283.723 466.102C302.487 480.585 327.555 486.459 352.217 479.852C386.997 470.532 410.068 439.312 410.555 405.006V317.717C410.555 315.08 409.125 312.621 406.843 311.303L374.292 292.509V407.145ZM251.868 415.897C248.296 417.959 243.893 417.959 240.317 415.897L155.526 366.942C152.366 390.436 159.811 415.08 177.866 433.136H177.863C203.325 458.594 241.896 462.962 271.85 446.232L347.449 402.586C349.735 401.268 351.148 398.8 351.148 396.163V358.579L251.868 415.897ZM368.602 220.628C366.319 219.309 363.474 219.318 361.191 220.637L328.641 239.431L427.921 296.749C431.496 298.811 433.697 302.627 433.697 306.752V404.661C455.622 395.654 473.244 376.881 479.851 352.218C489.169 317.442 473.668 281.85 444.201 264.274L368.602 220.628ZM177.303 206.34C155.377 215.348 137.756 234.122 131.148 258.783C121.832 293.561 137.331 329.153 166.799 346.727L242.398 390.373C244.68 391.692 247.525 391.684 249.807 390.366L282.357 371.572L183.078 314.253C179.504 312.189 177.303 308.375 177.303 304.251V206.34ZM259.849 279.145V331.858L305.5 358.213L351.15 331.858V279.145L305.5 252.789L259.849 279.145ZM327.276 144.9C308.512 130.418 283.445 124.543 258.782 131.15C224.002 140.471 200.931 171.691 200.445 205.995V293.286C200.445 295.923 201.875 298.381 204.158 299.7L236.707 318.493V203.856C236.707 199.731 238.909 195.916 242.483 193.853L327.276 144.9ZM433.137 177.867C407.675 152.407 369.103 148.038 339.149 164.769L263.55 208.415C261.265 209.734 259.852 212.202 259.852 214.838V252.423L359.132 195.105C362.703 193.041 367.108 193.041 370.682 195.105L455.473 244.06C458.635 220.567 451.189 195.922 433.135 177.867H433.137Z';

const CLAUDE_SPARK =
  'm4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z';

/** An agent's mark in its own colour, or nothing for an agent ket has no mark for.
 * `well` gives it a well's room, as a worktree's row does, so every row's
 * mark takes one size: clear until its session has a `/color` (protocol
 * 1.31), and that colour after, with the mark in whichever of the card and
 * the text stands off it more — see `sessionColor`. */
export function AgentMark({ agent, size = 14, session, well }: { agent: string; size?: number; session?: string; well?: boolean }) {
  const worn = sessionColor(session);
  const own = AGENT_COLORS[agentHarness(agent)] ?? t.dim;
  if (!worn && !well) return <AgentGlyph agent={agent} size={size} color={own} />;
  const side = size + 6;
  return (
    <View style={{ width: side, height: side, borderRadius: size > 14 ? 6 : 5, backgroundColor: worn?.tile, alignItems: 'center', justifyContent: 'center' }}>
      <AgentGlyph agent={agent} size={size} color={worn?.ink ?? own} />
    </View>
  );
}

/** An agent's logo alone, in one colour: its own, a session's, or a quiet
 * grey where the logo says only who the agent is. */
export function AgentGlyph({ agent, size, color }: { agent: string; size: number; color: string }) {
  const name = agentHarness(agent);
  if (name === 'codex') {
    // OpenAI's Blossom, as developers.openai.com/codex draws it.
    return (
      <Svg width={size} height={size} viewBox="100 100 411 411">
        <Path d={OPENAI_BLOSSOM} fill={color} fillRule="evenodd" clipRule="evenodd" />
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
    // OpenCode's mark, one colour, as opencode.ai/brand gives it.
    return (
      <Svg width={size} height={size} viewBox="-30 0 300 300">
        <Path d="M180 240H60V120H180V240Z" fill={color} fillOpacity={0.2} />
        <Path d="M180 60H60V240H180V60ZM240 300H0V0H240V300Z" fill={color} />
      </Svg>
    );
  }
  if (name === 'claude') {
    // Claude's spark, as Simple Icons 13.21.0 draws it (CC0).
    return (
      <Svg width={size} height={size} viewBox="0 0 24 24">
        <Path d={CLAUDE_SPARK} fill={color} />
      </Svg>
    );
  }
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24">
      <Circle cx={12} cy={12} r={5} fill={color} />
    </Svg>
  );
}

/** A stroke icon. `fill` fills what it outlines, for an icon that is chosen;
 * an icon of open lines has nothing to fill and leaves it out. */
function Stroke({ size = 18, color = t.dim, weight = 1.8, fill = 'none', children }: IconProps & { children: React.ReactNode }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill={fill} stroke={color} strokeWidth={weight} strokeLinecap="round" strokeLinejoin="round">
      {children}
    </Svg>
  );
}

/** Start: a sparkle, for setting an agent going. */
export const Sparkle = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z" />
    <Path d="M19 17l.7 1.8 1.8.7-1.8.7L19 22l-.7-1.8-1.8-.7 1.8-.7z" />
  </Stroke>
);

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

/** A time of day: an automation's start. */
export const Clock = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={12} cy={12} r={9} />
    <Path d="M12 7v5l3 2" />
  </Stroke>
);

/** Start now: a triangle on its side. */
export const Play = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M7 4.5v15l12-7.5z" />
  </Stroke>
);

export const Pause = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M8 5v14M16 5v14" />
  </Stroke>
);

export const Pencil = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M4 20l4-1 10-10-3-3L5 16z" />
  </Stroke>
);

export const Shield = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z" />
  </Stroke>
);

export const Bolt = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M13 3L5 13h6l-1 8 8-10h-6z" />
  </Stroke>
);

/** The local network: Wi-Fi arcs over a dot. */
export const Wifi = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 20h.01M8.94 17.43a4 4 0 0 1 6.12 0M5.49 14.54a8.5 8.5 0 0 1 13.02 0M2.04 11.64a13 13 0 0 1 19.92 0" />
  </Stroke>
);

/** Tailscale, or any tunnel to the Mac: the boxed VPN of the iPhone's own
 * status bar. Sized to sit among the stroke icons: 13 tall at 14. */
export const Vpn = ({ size = 18, color = t.dim }: IconProps) => (
  <View style={[vpn.box, { height: Math.round(size * 0.93), borderColor: color }]}>
    <Text allowFontScaling={false} style={[vpn.text, { fontSize: size * 0.57, color }]}>
      VPN
    </Text>
  </View>
);

const vpn = StyleSheet.create({
  box: { justifyContent: 'center', paddingHorizontal: 2, borderWidth: 1.3, borderRadius: 3 },
  text: { fontFamily: font.chromeSemibold, includeFontPadding: false },
});

/** A relay, ket's or another: a cloud of two round bumps on a flat base,
 * which stays clean at the pill's 14. */
export const Cloud = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M9.5 19a6.5 6.5 0 1 1 6.32-8H17a4 4 0 0 1 0 8z" />
  </Stroke>
);

/** An address someone typed in: two links of a chain. */
export const Link = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M10 13.5a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1.5 1.5" />
    <Path d="M14 10.5a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1.5-1.5" />
  </Stroke>
);

/** Each kind of address, as a glyph. */
export const ADDRESS_ICON: Record<AddressKind, (p: IconProps) => React.JSX.Element> = {
  local: Wifi,
  tailnet: Vpn,
  'ket-relay': Cloud,
  relay: Cloud,
};

export const Phone = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={7} y={2.5} width={10} height={19} rx={2.5} />
    <Path d="M11 18.5h2" />
  </Stroke>
);

export const Tablet = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={4.5} y={3} width={15} height={18} rx={2.5} />
    <Path d="M11 18h2" />
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

/** The worktree's changes: a plus over a minus, the two sides of a diff. */
/** More: what a screen offers beyond what is drawn on it, in its bar. */
export const More = (p: IconProps) => (
  <Stroke {...p} weight={2.6}>
    <Path d="M5 12h.01M12 12h.01M19 12h.01" />
  </Stroke>
);

export const Changes = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 3v14M5 10h14M5 21h14" />
  </Stroke>
);

/** A terminal to open: the prompt inside a window, so it has a body beside
 * a bordered control where the bare prompt looks thin. */
export const TerminalWindow = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={3} y={4} width={18} height={16} rx={2.5} />
    <Path d="m7 10 3 3-3 3M13 16h4" />
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

/** Throw away: a bin. */
export const Trash = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M4 7h16" />
    <Path d="M9.5 7V4.8a.8.8 0 0 1 .8-.8h3.4a.8.8 0 0 1 .8.8V7" />
    <Path d="m6.5 7 .8 11.6A2 2 0 0 0 9.3 20.5h5.4a2 2 0 0 0 2-1.9L17.5 7" />
    <Path d="M10.2 11v5.5M13.8 11v5.5" />
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

/** Dictate: a microphone. */
export const Mic = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={9} y={3} width={6} height={11} rx={3} />
    <Path d="M5.5 10.5a6.5 6.5 0 0 0 13 0" />
    <Path d="M12 17v4" />
  </Stroke>
);

/** A photo or a screenshot. */
export const Picture = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={3} y={3} width={18} height={18} rx={3} />
    <Circle cx={9} cy={9} r={1.8} />
    <Path d="m21 15-4.5-4.5L6 21" />
  </Stroke>
);

/** A file that is not a photo. */
export const FileGlyph = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
    <Path d="M14 3v5h5" />
  </Stroke>
);

/** Something to know: a circled i. */
export const Info = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={12} cy={12} r={9} />
    <Path d="M12 8v4.5" />
    <Path d="M12 16h.01" />
  </Stroke>
);

/** A heads-up, bare: an exclamation mark, for a mark that has its own well. */
export const Exclaim = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 6.5v7" />
    <Path d="M12 17.5h.01" />
  </Stroke>
);

/** By the way, bare: a lower-case i, for a mark that has its own well. */
export const Aside = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 10.5v7" />
    <Path d="M12 6.5h.01" />
  </Stroke>
);

/** End-to-end encrypted: a padlock, shut. */
export const Padlock = (p: IconProps) => (
  <Stroke {...p}>
    <Rect x={5} y={10.5} width={14} height={10} rx={2} />
    <Path d="M8 10.5V7.5a4 4 0 0 1 8 0v3" />
  </Stroke>
);

/** Hand failed checks to the agent: a tick in a ring. */
export const CheckCircle = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={12} cy={12} r={8.5} />
    <Path d="m8.5 12 2.5 2.5 4.5-5" />
  </Stroke>
);

/** Review comments: a speech bubble. */
export const Comment = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M4.5 5h15v10.5H10l-4.5 4v-4h-1z" />
  </Stroke>
);

/** A conversation: a speech bubble, with `dots` one under way. `dot` puts a
 * dot on its corner, as an unread row wears, cut out of the bubble by a ring
 * of `dot`: the colour the icon stands on. */
export const Bubble = ({ dots, dot, ...p }: IconProps & { dots?: boolean; dot?: string }) => (
  <Stroke {...p}>
    <Path d="M5 5h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-8l-4 3.5V17H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z" />
    {dots
      ? [8.5, 12, 15.5].map((cx) => <Circle key={cx} cx={cx} cy={11} r={1.2} fill={p.color ?? t.dim} stroke="none" />)
      : null}
    {dot ? <Circle cx={17.5} cy={6.5} r={3.2} fill={p.color ?? t.dim} stroke={dot} strokeWidth={1.6} /> : null}
  </Stroke>
);

/** Everything: the shell's `*`, which matches every name. Open lines, so
 * nothing to fill. */
export const Wildcard = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 4v16M5.1 8l13.8 8M18.9 8 5.1 16" />
  </Stroke>
);

/** Look again: one arrow round. */
export const Retry = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M20 12a8 8 0 1 1-2.34-5.66M20 4v5h-5" />
  </Stroke>
);

/** Bring a branch up to date: two arrows round. */
export const Refresh = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M19.5 11A7.5 7.5 0 0 0 6 6.5M4.5 3.5v4h4" />
    <Path d="M4.5 13A7.5 7.5 0 0 0 18 17.5M19.5 20.5v-4h-4" />
  </Stroke>
);

/** Frost: a session whose prompt cache has gone cold. */
export const Snowflake = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 2.5v19M3.8 7.25l16.4 9.5M3.8 16.75l16.4-9.5" />
    <Path d="M9.75 3.9 12 6l2.25-2.1M9.75 20.1 12 18l2.25 2.1" />
  </Stroke>
);

/** Merge: a branch joining its trunk. */
export const Merge = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={6} cy={5.5} r={2} />
    <Circle cx={6} cy={18.5} r={2} />
    <Circle cx={18} cy={12} r={2} />
    <Path d="M6 7.5v9M6 7.5c0 3 3 4.5 10 4.5" />
  </Stroke>
);

/** A pull request: a branch with its arrow back to the base. */
export const PullRequestGlyph = (p: IconProps) => (
  <Stroke {...p}>
    <Circle cx={6} cy={5.5} r={2} />
    <Circle cx={6} cy={18.5} r={2} />
    <Circle cx={18} cy={18.5} r={2} />
    <Path d="M6 7.5v9M18 16.5V9a3 3 0 0 0-3-3h-4" />
  </Stroke>
);

/** Pin a worktree to the top of its project: a pushpin. */
export const Pin = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 16.5V21M9 3.5h6l-1 5.5 3 3v2H7v-2l3-3z" />
  </Stroke>
);

/** Hide: an eye, struck through. */
export const EyeOff = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12z" />
    <Circle cx={12} cy={12} r={3} />
    <Path d="M4 4l16 16" />
  </Stroke>
);

/** Unpin: the pushpin, struck through. */
export const PinOff = (p: IconProps) => (
  <Stroke {...p}>
    <Path d="M12 16.5V21M9 3.5h6l-1 5.5 3 3v2H7v-2l3-3z" />
    <Path d="M4 4l16 16" />
  </Stroke>
);
