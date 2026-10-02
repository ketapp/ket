// Asking before something that cannot be undone. React Native's Alert does
// nothing in a browser, so there the page's own confirm() asks instead.

import { Alert, Platform } from 'react-native';

export function confirm(title: string, message: string, action: string): Promise<boolean> {
  if (Platform.OS === 'web') {
    return Promise.resolve(globalThis.confirm?.(`${title}\n\n${message}`) ?? false);
  }
  return new Promise((resolve) =>
    Alert.alert(title, message, [
      { text: 'Cancel', style: 'cancel', onPress: () => resolve(false) },
      { text: action, style: 'destructive', onPress: () => resolve(true) },
    ]),
  );
}
