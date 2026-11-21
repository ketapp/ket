// Styles more than one family of the app's controls draws with.

import { StyleSheet } from 'react-native';

import { font, size, t } from '../../lib/theme';

export const shared = StyleSheet.create({
  count: { fontFamily: font.chrome, fontSize: 12, color: t.faint },
  rowText: { flex: 1, minWidth: 0, gap: 3 },
  detail: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  segmentOn: { backgroundColor: t.selection },
  readout: {
    height: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 9,
    borderRadius: size.radiusSm,
    backgroundColor: t.sunken,
  },
});
