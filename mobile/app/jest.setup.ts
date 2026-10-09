// Shared jest setup: the native libraries' own jest mocks, as their docs give them.
import { jest } from '@jest/globals';
import { setUpTests } from 'react-native-reanimated';

// react-native-worklets (docs: guides/testing) — reanimated runs on it.
// Hoisted above the imports, so reanimated loads against the mock.
jest.mock('react-native-worklets', () => jest.requireActual('react-native-worklets/src/mock'));

// react-native-keyboard-controller ships its mock at `/jest`.
jest.mock('react-native-keyboard-controller', () => jest.requireActual('react-native-keyboard-controller/jest'));

// react-native-reanimated (docs: guides/testing).
setUpTests();
