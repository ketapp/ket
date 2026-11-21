// Small marks: a project's badge with its glyph, tags and dots.

import { type ReactNode } from 'react';
import { StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';
import Svg, { Circle, Ellipse, G, Line, Path, Polygon, Polyline, Rect } from 'react-native-svg';

import { GLYPHS, type Glyph, type GlyphPart } from '../../lib/glyphs';
import { font, size, t, tint } from '../../lib/theme';

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

export function Dot({ color, size: side = 7 }: { color: string; size?: number }) {
  return <View style={{ width: side, height: side, borderRadius: side / 2, backgroundColor: color }} />;
}

const styles = StyleSheet.create({
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
