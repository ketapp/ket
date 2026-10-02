// The app's version, from app.json — the one place it is kept. `version` is
// the release version people see (bumped for each App Store release);
// `build` counts uploads of it (iOS `buildNumber`, Android `versionCode`).
// In Expo Go these still come from app.json, through the dev server.

import Constants from 'expo-constants';
import { Platform } from 'react-native';

const config = Constants.expoConfig;

export const version: string = config?.version ?? '0.0.0';

export const build: string =
  (Platform.OS === 'android' ? config?.android?.versionCode?.toString() : config?.ios?.buildNumber) ?? '0';

/** "0.1.0 (1)" — how the app names itself to a person and to the desktop. */
export const versionLabel = `${version} (${build})`;
