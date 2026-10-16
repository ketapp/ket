// ⌘Return, from a hardware keyboard, on whatever is focused inside: an iPad
// with a keyboard sends with it, as a Mac does, while Return alone still
// starts a new line. The native half is modules/ket-keys; a phone without a
// keyboard never sees it.

import { requireNativeView, requireOptionalNativeModule } from 'expo';
import type { ComponentType, ReactNode } from 'react';
import { Platform, View, type StyleProp, type ViewStyle } from 'react-native';

interface Props {
  /** ⌘Return. */
  onSend(): void;
  /** What holding ⌘ lists the shortcut as. */
  title: string;
  style?: StyleProp<ViewStyle>;
  children: ReactNode;
}

type NativeProps = { title: string; onSend(): void; style?: StyleProp<ViewStyle>; children: ReactNode };

/** Missing from a build made before the module was: the children are drawn
 * without the shortcut rather than not at all. */
const Native: ComponentType<NativeProps> | null = (() => {
  if (Platform.OS !== 'ios' || !requireOptionalNativeModule('KetKeys')) return null;
  return requireNativeView<NativeProps>('KetKeys');
})();

export function KeyCommands({ onSend, title, style, children }: Props) {
  if (!Native) return <View style={style}>{children}</View>;
  return (
    <Native title={title} onSend={onSend} style={style}>
      {children}
    </Native>
  );
}
