// The rows of a chat that are not turns read from the transcript: a message
// of yours that the conversation has not shown yet, and the agent's working
// row. Drawn by the session's chat, and by the chat that stands in for one
// while the desktop is still making its worktree — see app/starting.

import { useEffect, useId, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, useWindowDimensions, View, type TextInputProps, type ViewStyle } from 'react-native';
import Animated, {
  cancelAnimation,
  Easing,
  FadeInDown,
  ReduceMotion,
  type SharedValue,
  useAnimatedProps,
  useAnimatedStyle,
  useFrameCallback,
  useReducedMotion,
  useSharedValue,
  withRepeat,
  withTiming,
} from 'react-native-reanimated';
import Svg, { Circle, Defs, LinearGradient, Path, Stop, Text as SvgText } from 'react-native-svg';

import { fileName, type Attachment } from '../lib/attach';
import { agentLabel, blend, font, size, t, tint } from '../lib/theme';
import { FileChip, SentFiles } from './Attachments';
import { AgentMark } from './icons';
import { Slide } from './Swap';

const AnimatedCircle = Animated.createAnimatedComponent(Circle);
const AnimatedGradient = Animated.createAnimatedComponent(LinearGradient);
const AnimatedInput = Animated.createAnimatedComponent(TextInput);
/** The ring's circumference, in its 22pt view box. */
const ORBIT_LENGTH = 2 * Math.PI * 10;
const BEAT_MS = 850;
const SHINE = blend(t.accent, t.accentPressed, 0.35);
const motion = { reduceMotion: ReduceMotion.System };
/** The details fall four points into place, below the working words. */
const DETAILS_IN = FadeInDown.duration(200)
  .easing(Easing.bezier(0.2, 0.8, 0.2, 1))
  .withInitialValues({ opacity: 0, transform: [{ translateY: -4 }] })
  .reduceMotion(ReduceMotion.System);

/** Where a message of yours stands. `held`: the desktop has it, and the
 * agent it is for is not running yet. */
export type OutgoingStatus = 'sending' | 'queued' | 'unseen' | 'held';

/** A message of yours on its way. */
export function Outgoing({
  text,
  files = [],
  named = [],
  status,
  onCopy,
  onDismiss,
}: {
  text: string;
  /** Photos and files sent with it, drawn above its words. */
  files?: Attachment[];
  /** The paths of files the desktop listed under it, for one this phone
   * holds no files for. */
  named?: string[];
  status: OutgoingStatus;
  onCopy: (text: string) => void;
  onDismiss?: () => void;
}) {
  const said =
    status === 'queued'
      ? 'Queued, read when the agent finishes this step'
      : status === 'unseen'
        ? 'Not in the conversation, check the terminal'
        : status === 'held'
          ? 'Waiting on your Mac'
          : 'Sending';
  const faded = status === 'sending' || status === 'held';
  const words = <Text style={[styles.youText, faded && styles.youSendingText]}>{text}</Text>;
  const names = files.length > 0 ? files.map((file) => file.name) : named.map(fileName);
  const what = text || (names.length === 1 ? names[0] : `${names.length} files`);
  return (
    <Pressable
      onPress={status === 'unseen' ? onDismiss : undefined}
      delayLongPress={350}
      onLongPress={() => onCopy(text)}
      accessibilityLabel={`${what}. ${said}.`}
      accessibilityHint={status === 'unseen' ? 'Tap to clear, hold to copy' : 'Hold to copy'}
      style={({ pressed }) => [
        names.length > 0 ? styles.yours : [styles.you, faded && styles.youSending],
        pressed && styles.held,
      ]}
    >
      {names.length > 0 ? (
        <>
          {files.length > 0 ? <SentFiles files={files} sending={faded} /> : named.map((path) => <FileChip key={path} name={fileName(path)} faded={faded} />)}
          {text ? (
            <View style={[styles.bubble, faded && styles.youSending]}>
              <StatusMark status={status} />
              {words}
            </View>
          ) : (
            <StatusMark status={status} />
          )}
        </>
      ) : (
        <>
          <StatusMark status={status} />
          {words}
        </>
      )}
    </Pressable>
  );
}

/**
 * Where a message of yours stands, as a mark in the gutter to the left of its
 * bubble: a dashed ring while it is sent, an amber clock while the agent — or
 * the desktop, for a message to an agent not running yet — holds it, a red !
 * when it never arrived, a faint tick once it is in.
 *
 * Centred on the bubble's first line, however many lines the message runs to,
 * and drawn outside the bubble's box: it never shifts the text, never changes
 * where a long message wraps, and changing from one mark to the next — or the
 * tick moving on to the next message — moves nothing else. Shape as well as
 * colour tells the marks apart.
 */
export function StatusMark({ status }: { status: OutgoingStatus | 'delivered' }) {
  return (
    <View pointerEvents="none" style={styles.statusMark}>
      <Svg width={14} height={14} viewBox="0 0 14 14">
        {status === 'sending' ? (
          <Circle cx={7} cy={7} r={5.25} fill="none" stroke={t.faint} strokeWidth={1.5} strokeDasharray="3 2.5" />
        ) : status === 'queued' || status === 'held' ? (
          <>
            <Circle cx={7} cy={7} r={5.25} fill="none" stroke={t.quotaWarm} strokeWidth={1.5} />
            <Path d="M7 4.2V7l2 1.3" fill="none" stroke={t.quotaWarm} strokeWidth={1.5} strokeLinecap="round" />
          </>
        ) : status === 'unseen' ? (
          <>
            <Circle cx={7} cy={7} r={6} fill={t.failed} />
            <Path d="M7 3.8v3.8" stroke={t.backdrop} strokeWidth={1.6} strokeLinecap="round" />
            <Circle cx={7} cy={9.9} r={0.95} fill={t.backdrop} />
          </>
        ) : (
          <Path
            d="M3.5 7.3 5.9 9.6 10.5 4.6"
            fill="none"
            stroke={t.faint}
            strokeWidth={1.6}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        )}
      </Svg>
    </View>
  );
}

/** The agent at work: a ring orbiting its mark, and what it is doing — or what
 * the desktop is doing to get it running. Only the chat opens details. */
export function Working({ agent, what, details = false }: { agent: string; what: string; details?: boolean }) {
  const still = useReducedMotion();
  const [expanded, setExpanded] = useState(false);
  // Kept as the words change, from this phone's first sight of the turn.
  const [began] = useState(Date.now);
  const beat = useSharedValue(0);
  const arc = useSharedValue(0.14);
  const shine = useSharedValue(0);
  const words = what || 'Waiting for the agent';
  const name = agent ? agentLabel(agent) : 'Agent';

  useEffect(() => {
    beat.set(0);
    arc.set(0.14);
    shine.set(0);
    if (still) return;
    beat.set(withRepeat(withTiming(1, { duration: BEAT_MS, easing: Easing.linear, ...motion }), -1));
    arc.set(withRepeat(withTiming(0.46, { duration: BEAT_MS, easing: Easing.bezier(0.42, 0, 0.58, 1), ...motion }), -1, true));
    shine.set(withRepeat(withTiming(1, { duration: 1500, easing: Easing.linear, ...motion }), -1));
    return () => {
      cancelAnimation(beat);
      cancelAnimation(arc);
      cancelAnimation(shine);
    };
  }, [still, beat, arc, shine]);
  const orbit = useAnimatedStyle(() => ({ transform: [{ rotate: `${beat.get() * 360}deg` }] }));
  const breathing = useAnimatedProps(() => ({
    strokeDasharray: [ORBIT_LENGTH * arc.get(), ORBIT_LENGTH * (1 - arc.get())],
  }));
  const row = (
    <View
      style={styles.working}
      pointerEvents="none"
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
    >
      <View style={styles.orbit}>
        <Animated.View style={[StyleSheet.absoluteFill, orbit]}>
          <Svg width={22} height={22} viewBox="0 0 22 22">
            <Circle cx={11} cy={11} r={10} fill="none" stroke={t.border} strokeWidth={1.6} />
            <AnimatedCircle
              cx={11} cy={11} r={10} fill="none" stroke={t.running} strokeWidth={1.6}
              strokeLinecap="round" animatedProps={breathing}
            />
          </Svg>
        </Animated.View>
        {agent ? <AgentMark agent={agent} size={11} /> : null}
      </View>
      <Slide token={words} item={words} style={styles.workingWords}>
        {(text) => still ? (
          <Text style={styles.workingText} numberOfLines={1}>{text}</Text>
        ) : (
          <WorkingWords what={text} shine={shine} />
        )}
      </Slide>
    </View>
  );
  if (!details) return <View accessible accessibilityLabel={`${name} is working: ${words}`}>{row}</View>;
  return (
    <View>
      <Pressable
        onPress={() => setExpanded((open) => !open)}
        accessibilityRole="button"
        accessibilityState={{ expanded }}
        accessibilityLabel={`${name} is working: ${words}. ${expanded ? 'Hide' : 'Show'} details`}
        hitSlop={{ top: 11, bottom: 11 }}
        style={({ pressed }) => pressed && styles.held}
      >
        {row}
      </Pressable>
      {expanded ? (
        <Animated.View entering={still ? undefined : DETAILS_IN} style={styles.workingDetails}>
          <Text style={styles.workingDetailText}>{words}</Text>
          <WorkingElapsed began={began} />
        </Animated.View>
      ) : null}
    </View>
  );
}

/** A clock read on the UI thread, including time spent away from the app.
 * Only mounted while the details are open; Reduce Motion still tells time. */
function WorkingElapsed({ began }: { began: number }) {
  const { fontScale } = useWindowDimensions();
  const [first] = useState(() => Math.max(0, Math.floor((Date.now() - began) / 1000)));
  const seconds = useSharedValue(first);
  useFrameCallback(() => {
    const next = Math.max(0, Math.floor((Date.now() - began) / 1000));
    if (next !== seconds.get()) seconds.set(next);
  });
  const clock = useAnimatedProps<TextInputProps & { text: string }>(() => {
    const value = seconds.get();
    const text = `${Math.floor(value / 60)}m ${String(value % 60).padStart(2, '0')}s`;
    return { text, defaultValue: text, accessibilityLabel: `Elapsed time: ${text}` };
  });
  return (
    <AnimatedInput
      editable={false}
      caretHidden
      pointerEvents="none"
      accessibilityRole="text"
      underlineColorAndroid="transparent"
      animatedProps={clock}
      style={[styles.workingElapsed, { width: 82 * fontScale }]}
    />
  );
}

/** SVG gives the words a moving band of ink, with no native masking module. */
function WorkingWords({ what, shine }: { what: string; shine: SharedValue<number> }) {
  const { fontScale } = useWindowDimensions();
  const [width, setWidth] = useState(0);
  return (
    <View style={styles.workingWords} onLayout={(e) => setWidth(e.nativeEvent.layout.width)}>
      {width > 0 ? <FittedWorkingWords key={`${width}:${fontScale}:${what}`} what={what} width={width} fontScale={fontScale} shine={shine} /> : null}
    </View>
  );
}

/** Native text measures the actual font, including wide glyphs. Long words
 * are shortened by whole characters until their ellipsis fits the column.
 * Measurement happens only when the words or the column's width change;
 * the gradient's movement stays on the UI thread. */
function FittedWorkingWords({ what, width, fontScale, shine }: { what: string; width: number; fontScale: number; shine: SharedValue<number> }) {
  const id = useId();
  const letters = Array.from(what.replace(/\s+/g, ' '));
  const [fit, setFit] = useState({ low: 0, high: letters.length, at: letters.length });
  const [measured, setMeasured] = useState<{ words: string; baseline: number } | null>(null);
  const candidate = letters.slice(0, fit.at).join('') + (fit.at < letters.length ? '…' : '');
  const gradient = useAnimatedProps(() => ({
    x1: width * (shine.get() * 2 - 1),
    x2: width * shine.get() * 2,
  }));
  return (
    <View style={styles.workingWords}>
      {measured ? (
        <Svg width={width} height={22} accessible={false}>
          <Defs>
            <AnimatedGradient id={id} gradientUnits="userSpaceOnUse" y1={0} y2={0} animatedProps={gradient}>
              <Stop offset="0" stopColor={t.dim} />
              <Stop offset="0.38" stopColor={t.dim} />
              <Stop offset="0.5" stopColor={SHINE} />
              <Stop offset="0.62" stopColor={t.dim} />
              <Stop offset="1" stopColor={t.dim} />
            </AnimatedGradient>
          </Defs>
          <SvgText x={0} y={measured.baseline} fontFamily={font.chrome} fontSize={size.detail * fontScale} fill={`url(#${id})`}>
            {measured.words}
          </SvgText>
        </Svg>
      ) : (
        <Text style={styles.workingText} numberOfLines={1}>{what}</Text>
      )}
      {!measured ? (
        <View pointerEvents="none" accessible={false} style={styles.workingMeasure}>
          <Text
            style={styles.workingText}
            onTextLayout={({ nativeEvent: { lines } }) => {
              const line = lines[0];
              if (!line) return;
              const fits = line.width <= width;
              const low = fits ? fit.at : fit.low;
              const high = fits ? fit.high : fit.at - 1;
              if (low >= high) {
                setMeasured({
                  words: letters.slice(0, low).join('') + (low < letters.length ? '…' : ''),
                  baseline: line.y + line.ascender,
                });
              } else {
                setFit({ low, high, at: Math.ceil((low + high) / 2) });
              }
            }}
          >
            {candidate}
          </Text>
        </View>
      ) : null}
    </View>
  );
}

/** A message of yours: a bubble on the right, its tail at the foot. */
export const bubble: ViewStyle = {
  backgroundColor: t.selection,
  borderRadius: 18,
  borderBottomRightRadius: 6,
  paddingHorizontal: 12,
  paddingVertical: 9,
  gap: 6,
};

const styles = StyleSheet.create({
  you: { ...bubble, alignSelf: 'flex-end', maxWidth: '86%' },
  // A message of yours with photos or files: them, then its bubble.
  yours: { alignSelf: 'flex-end', maxWidth: '86%', alignItems: 'flex-end', gap: 4 },
  bubble,
  // Faded while sending — the fill and the words, not the mark beside them —
  // at the delivered bubble's size and corners.
  youSending: { backgroundColor: tint(t.selection, 0.7) },
  youSendingText: { opacity: 0.7 },
  // In the gutter left of the bubble, centred on the first line: the bubble's
  // top padding, then half of a 20pt line less half the 14pt mark.
  statusMark: { position: 'absolute', left: -20, top: 12 },
  youText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 20, color: t.text },
  held: { opacity: 0.7 },
  working: { height: 22, flexDirection: 'row', alignItems: 'center', gap: 10 },
  // A 22pt ring in the reply's 14pt mark column: words still start at 24pt.
  orbit: { width: 22, height: 22, marginHorizontal: -4, alignItems: 'center', justifyContent: 'center' },
  workingWords: { flex: 1, minWidth: 0, height: 22, overflow: 'hidden' },
  workingText: { fontFamily: font.chrome, fontSize: size.detail, lineHeight: 22, color: t.dim, includeFontPadding: false },
  // Wide enough to measure a single line, outside the visible row's layout.
  workingMeasure: { position: 'absolute', width: 100_000, opacity: 0 },
  workingDetails: { marginTop: 4, marginLeft: 24, flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
  workingDetailText: { flex: 1, minWidth: 0, fontFamily: font.chrome, fontSize: 12, lineHeight: 18, color: t.faint },
  workingElapsed: { padding: 0, fontFamily: font.chrome, fontSize: 12, lineHeight: 18, color: t.faint, textAlign: 'right', includeFontPadding: false },
});
