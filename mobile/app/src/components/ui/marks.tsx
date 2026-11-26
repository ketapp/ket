// Small marks: a project's badge with its glyph, an agent's tile, tags,
// chips and dots.

import { type ReactNode } from 'react';
import { StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';
import Svg, { Circle, Ellipse, G, Line, Path, Polygon, Polyline, Rect } from 'react-native-svg';

import { GLYPHS, type Glyph, type GlyphPart } from '../../lib/glyphs';
import { font, sessionColor, size, t, tint } from '../../lib/theme';
import { AgentGlyph, TerminalGlyph } from '../icons';

/** The glyph a project's icon names (`glyph:<name>`, from a desktop at 1.27
 * or later), if this phone has it. A name it does not have draws the letter,
 * as the desktop does for a glyph a later ket dropped. */
export function glyphOf(icon: string | undefined): Glyph | undefined {
  const name = icon?.trim().match(/^glyph:(.+)$/)?.[1];
  // Its own entries only: `glyph:constructor` typed on the desktop is text
  // there, and must not find the object's prototype here.
  return name && Object.prototype.hasOwnProperty.call(GLYPHS, name) ? GLYPHS[name] : undefined;
}

/** A project's badge, in its colour on its colour at 13%: the glyph the
 * desktop gave it, else the icon typed there, else its initial. */
export function Badge({
  name,
  color,
  icon,
  size: side = 22,
}: {
  name: string;
  color: string;
  icon?: string;
  size?: number;
}) {
  const ink = lift(color);
  const glyph = glyphOf(icon);
  // A glyph this phone lacks is no icon at all, rather than its name spelled out.
  const typed = glyph || icon?.trim().startsWith('glyph:') ? '' : (icon?.trim() ?? '');
  return (
    <View style={[styles.badge, { backgroundColor: tint(color, 0.13), width: side, height: side }]}>
      {glyph ? (
        // Fluent's filled shapes on the glyph's own grid (16 units for most,
        // 20 for a few), drawn at two thirds of the badge, as solid as the
        // letter it stands in for.
        <Svg width={side * 0.64} height={side * 0.64} viewBox={glyph.viewBox} testID="badge-glyph">
          <G fill={ink}>
            {glyph.parts.map((part, at) => (
              <GlyphShape key={at} part={part} />
            ))}
          </G>
        </Svg>
      ) : (
        <Text style={[styles.badgeText, { color: ink }]} numberOfLines={1}>
          {typed || (name.trim()[0] ?? '?').toUpperCase()}
        </Text>
      )}
    </View>
  );
}

/** One shape of a glyph, as react-native-svg draws it. */
function GlyphShape({ part }: { part: GlyphPart }) {
  switch (part.kind) {
    case 'path':
      return <Path d={part.d} />;
    case 'circle':
      return <Circle cx={part.cx} cy={part.cy} r={part.r} />;
    case 'ellipse':
      return <Ellipse cx={part.cx} cy={part.cy} rx={part.rx} ry={part.ry} />;
    case 'rect':
      return <Rect x={part.x} y={part.y} width={part.width} height={part.height} rx={part.rx} ry={part.ry} />;
    case 'line':
      return <Line x1={part.x1} y1={part.y1} x2={part.x2} y2={part.y2} />;
    case 'polyline':
      return <Polyline points={part.points} />;
    case 'polygon':
      return <Polygon points={part.points} />;
  }
}

/** A project colour made light enough to read on the dark card. */
function lift(hex: string): string {
  const n = parseInt(hex.slice(1), 16);
  const lum = 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255);
  return lum < 90 ? t.dim : hex;
}

/** A quiet tag in a well: a word or a count. Given a `color`, it carries
 * that state's own hue on a faint ground of it instead — the desktop's
 * tinted tag, as on "Beta". */
export function Tag({
  children,
  mono,
  color,
  style,
}: {
  children: ReactNode;
  mono?: boolean;
  color?: string;
  style?: StyleProp<TextStyle>;
}) {
  return (
    <Text
      style={[
        styles.tag,
        mono && { fontFamily: font.chrome, fontVariant: ['tabular-nums'] },
        color ? { color, backgroundColor: tint(color, 0.16) } : null,
        style,
      ]}
    >
      {children}
    </Text>
  );
}

/** A dot in a state's colour. `hollow` is a ring of it: the state is not
 * there now — a desktop that is offline, or still being reached. */
export function Dot({ color, size: side = 7, hollow }: { color: string; size?: number; hollow?: boolean }) {
  return (
    <View
      style={[
        { width: side, height: side, borderRadius: side / 2 },
        hollow ? { borderWidth: 1.5, borderColor: color } : { backgroundColor: color },
      ]}
    />
  );
}

/** An agent tile's side: a project badge's on the desk, and a row's mark. */
export const TILE = 24;

/**
 * A row's mark: what it is, on a raised tile, and how it is going as a dot
 * on the tile's corner — none while it is quiet. `ring` is the colour round
 * the dot: what the tile stands on. `fill` stands in for the raised fill, as
 * a session's colour does; `faded` is something kept but not running.
 */
export function Tile({
  children,
  tone,
  ring = t.card,
  fill,
  faded,
  label,
  size: side = TILE,
}: {
  children: ReactNode;
  /** The state's colour, for the dot. */
  tone?: string;
  ring?: string;
  fill?: string;
  faded?: boolean;
  /** What a screen reader calls it: the thing and its state. */
  label?: string;
  /** Its side: a row's 24, or a project badge's 20 where it heads a card. */
  size?: number;
}) {
  return (
    <View
      accessible={label !== undefined}
      accessibilityLabel={label}
      style={[
        styles.tile,
        { width: side, height: side, borderRadius: Math.round(side * 0.29), backgroundColor: fill ?? t.elevated },
        faded && styles.tileFaded,
      ]}
    >
      {children}
      {tone ? <View style={[styles.tileDot, { backgroundColor: tone, borderColor: ring }]} /> : null}
    </View>
  );
}

/** An icon's size on a tile `side` across: 14 on a row's 24. */
export function tileGlyph(side: number = TILE): number {
  return Math.round(side * 0.58);
}

/**
 * Who works in a row, and how it is going, said once: the agent's logo on a
 * {@link Tile}, in its state's colour, and the state as the tile's dot. With
 * no agent, a terminal's prompt stands in. A session's `/color` (protocol
 * 1.31) fills the tile, as `sessionColor` gives it.
 */
export function AgentTile({
  agent,
  session,
  tone,
  ring = t.card,
  faded,
  label,
  size: side = TILE,
}: {
  agent: string;
  session?: string;
  /** The state's colour, for the dot and the logo. */
  tone?: string;
  ring?: string;
  faded?: boolean;
  /** What a screen reader calls it: the agent and its state. */
  label?: string;
  /** Its side: a row's 24, or a project badge's 20 where it heads a card. */
  size?: number;
}) {
  const worn = sessionColor(session);
  const glyph = tileGlyph(side);
  // The logo lights in its state's colour; on a session's colour, it keeps
  // that tile's ink, and the dot says the state.
  const ink = worn?.ink ?? tone ?? t.dim;
  return (
    <Tile tone={tone} ring={ring} fill={worn?.tile} faded={faded} label={label} size={side}>
      {agent ? <AgentGlyph agent={agent} size={glyph} color={ink} /> : <TerminalGlyph size={glyph + 1} color={ink} weight={2} />}
    </Tile>
  );
}

/**
 * How much of a plan's window is used, as a ring a tile's size: a track of
 * the raised fill, and an arc in `color` from the top round to `share` (0
 * to 1). It stands in a row's mark slot, where an agent's tile would.
 */
export function Ring({ share, color, label, size: side = TILE }: { share: number; color: string; label?: string; size?: number }) {
  const stroke = side / 8;
  const r = side / 2 - stroke / 2 - 1;
  const round = 2 * Math.PI * r;
  const arc = (Number.isFinite(share) ? Math.min(1, Math.max(0, share)) : 0) * round;
  const mid = side / 2;
  return (
    <View accessible={label !== undefined} accessibilityLabel={label}>
      <Svg width={side} height={side}>
        <Circle cx={mid} cy={mid} r={r} fill="none" stroke={t.elevated} strokeWidth={stroke} />
        {arc > 0 ? (
          <Circle
            cx={mid}
            cy={mid}
            r={r}
            fill="none"
            stroke={color}
            strokeWidth={stroke}
            strokeLinecap="round"
            strokeDasharray={[arc, round]}
            transform={`rotate(-90 ${mid} ${mid})`}
          />
        ) : null}
      </Svg>
    </View>
  );
}

/** A figure on a small capsule of the raised fill, led by a glyph: a pull
 * request's number, its glyph in its checks' colour. */
export function Chip({ icon, children, label }: { icon?: ReactNode; children: ReactNode; label?: string }) {
  return (
    <View accessible={label !== undefined} accessibilityLabel={label} style={styles.chip}>
      {icon}
      <Text style={styles.chipText}>{children}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  tile: { alignItems: 'center', justifyContent: 'center' },
  tileFaded: { opacity: 0.5 },
  // 10 across with a 2 pt ring of the ground, 3 out past the tile's corner.
  tileDot: { position: 'absolute', right: -5, bottom: -5, width: 14, height: 14, borderRadius: 7, borderWidth: 2 },
  chip: {
    height: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingLeft: 7,
    paddingRight: 8,
    borderRadius: 12,
    backgroundColor: t.elevated,
  },
  chipText: { fontFamily: font.chromeMedium, fontSize: 12, color: t.dim, fontVariant: ['tabular-nums'] },
  badge: { borderRadius: size.radiusSm, alignItems: 'center', justifyContent: 'center' },
  badgeText: { fontFamily: font.chromeSemibold, fontSize: 11.5 },
  tag: {
    fontFamily: font.prose,
    fontSize: 11.5,
    color: t.dim,
    backgroundColor: t.sunken,
    paddingHorizontal: 7,
    paddingVertical: 2,
    borderRadius: size.radiusSm,
    overflow: 'hidden',
    alignSelf: 'flex-start',
  },
});
