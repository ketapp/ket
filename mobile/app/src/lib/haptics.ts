// The tap a held row gives as its menu opens, as iOS's own context menus do.

import * as Haptics from 'expo-haptics';

/** A held row's menu opening: one medium tap. Nothing where the device has
 * no haptics, or they are turned off. */
export function menuOpened(): void {
  void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
}
