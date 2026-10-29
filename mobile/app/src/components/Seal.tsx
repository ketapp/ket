// The lock set into the end of the home strip's host capsule: a padlock whose
// shackle hangs open and dim until the phone's Noise handshake with the
// desktop completes, then drops shut and turns running green. It plays once
// per handshake — every connect and reconnect, so every launch — and never
// on a timer: the motion is the state changing, not decoration. Under Reduce
// Motion it is simply open or shut.

import { useEffect, useState } from 'react';
import { Animated, Easing } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import Svg, { Path, Rect } from 'react-native-svg';

import { t } from '../lib/theme';

const AnimatedPath = Animated.createAnimatedComponent(Path);
const AnimatedRect = Animated.createAnimatedComponent(Rect);

/** How long the shackle takes to drop, with a little bounce as it lands. */
const SHUT_MS = 420;
/** How long it takes to lift when the link goes. */
const OPEN_MS = 180;
/** How far the open shackle is lifted and tilted, in the icon's 24 grid. */
const LIFT = -3.5;
const TILT = -14;

export function Seal({ sealed, size = 13 }: { sealed: boolean; size?: number }) {
  const still = useReducedMotion();
  // Mounted already sealed, it is shut from the first frame: the motion is for
  // a handshake the person was there for, not for a screen opened later.
  const [shut] = useState(() => new Animated.Value(sealed ? 1 : 0));

  useEffect(() => {
    if (still) {
      shut.setValue(sealed ? 1 : 0);
      return;
    }
    const run = Animated.timing(shut, {
      toValue: sealed ? 1 : 0,
      duration: sealed ? SHUT_MS : OPEN_MS,
      easing: sealed ? Easing.out(Easing.back(2.2)) : Easing.out(Easing.quad),
      useNativeDriver: false,
    });
    run.start();
    return () => run.stop();
  }, [sealed, still, shut]);

  // The ink turns as the shackle lands, not before; the bounce past 1 keeps
  // the colour clamped and only moves the shackle.
  const ink = shut.interpolate({ inputRange: [0, 0.8, 1], outputRange: [t.dim, t.dim, t.running], extrapolate: 'clamp' });
  const lift = shut.interpolate({ inputRange: [0, 1], outputRange: [LIFT, 0] });
  const tilt = shut.interpolate({ inputRange: [0, 1], outputRange: [TILT, 0] });

  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      <AnimatedRect x={5} y={10.5} width={14} height={10} rx={2} stroke={ink} />
      <AnimatedPath d="M8 10.5V7.5a4 4 0 0 1 8 0v3" stroke={ink} originX={12} originY={10.5} translateY={lift} rotation={tilt} />
    </Svg>
  );
}
