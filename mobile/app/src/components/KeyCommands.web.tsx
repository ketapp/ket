// ⌘Return (Ctrl+Return off a Mac) in a browser: see KeyCommands.tsx.

import { type ReactNode, useEffect, useEffectEvent, useRef } from 'react';
import { View, type StyleProp, type ViewStyle } from 'react-native';

interface Props {
  onSend(): void;
  title: string;
  style?: StyleProp<ViewStyle>;
  children: ReactNode;
}

export function KeyCommands({ onSend, style, children }: Props) {
  const box = useRef<View>(null);
  const send = useEffectEvent(onSend);
  useEffect(() => {
    // A View is a DOM element in a browser.
    const node = box.current as unknown as HTMLElement | null;
    if (!node) return;
    const down = (event: KeyboardEvent) => {
      if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey) || event.isComposing) return;
      event.preventDefault();
      send();
    };
    node.addEventListener('keydown', down);
    return () => node.removeEventListener('keydown', down);
  }, []);
  return (
    <View ref={box} style={style}>
      {children}
    </View>
  );
}
