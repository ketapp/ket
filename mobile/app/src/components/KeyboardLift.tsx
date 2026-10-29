// What react-native-keyboard-controller's KeyboardStickyView does — ride up
// with the keyboard, by its height plus an offset — as a layout offset
// (`top`) instead of a transform.
//
// A transform moves what is drawn but not where React Native thinks the
// view is, and a button's press is measured against the latter: a tap that
// moved a pixel was judged to have slid off and was dropped. The library
// resyncs that position when the keyboard stops moving (its #1633, in
// 1.22.5), but any render after — a key typed into the composer — throws it
// back, so the first tap on Send or + after typing did nothing and the
// second worked. `top` is laid out, so where a button is drawn and where it
// is pressed cannot drift apart. Like a transform, it moves nothing around
// the view. Swap back once the library keeps the two in sync across renders.

import type { ReactNode } from 'react';
import type { StyleProp, ViewStyle } from 'react-native';
import { useReanimatedKeyboardAnimation } from 'react-native-keyboard-controller';
import Animated, { interpolate, type SharedValue, useAnimatedStyle, useDerivedValue } from 'react-native-reanimated';

/** Added to the lift with the keyboard closed and open, as
 * KeyboardStickyView's `offset`. */
export interface LiftOffset {
  closed?: number;
  opened?: number;
}

/** The `top` a KeyboardLift with this offset is at, frame for frame — for
 * something outside it that has to move with what it lifts. */
export function useLift({ closed = 0, opened = 0 }: LiftOffset = {}): SharedValue<number> {
  const { height, progress } = useReanimatedKeyboardAnimation();
  return useDerivedValue(() => height.get() + interpolate(progress.get(), [0, 1], [closed, opened]), [closed, opened]);
}

export function KeyboardLift({
  offset,
  style,
  children,
}: {
  offset?: LiftOffset;
  /** Not absolutely placed: `top` would stretch a view pinned by `bottom`. */
  style?: StyleProp<ViewStyle>;
  children: ReactNode;
}) {
  const top = useLift(offset);
  const lifted = useAnimatedStyle(() => ({ top: top.get() }));
  return <Animated.View style={[style, lifted]}>{children}</Animated.View>;
}
