import { describe, expect, jest, test } from '@jest/globals';
import { build, version, versionLabel } from './version';

jest.mock('expo-constants', () => ({
  __esModule: true,
  default: { expoConfig: { version: '1.2.3', ios: { buildNumber: '45' }, android: { versionCode: 67 } } },
}));

describe('version labels', () => {
  test('uses app config release and build versions', () => {
    expect(version).toBe('1.2.3');
    expect(['45', '67']).toContain(build);
    expect(versionLabel).toBe(`1.2.3 (${build})`);
  });
});
