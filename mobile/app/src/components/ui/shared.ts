// Styles more than one family of the app's controls draws with.

import { StyleSheet } from 'react-native';

import { font, size, t } from '../../lib/theme';
import { surface } from './surface';

export const shared = StyleSheet.create({
  count: { fontFamily: font.chrome, fontSize: 12, color: t.faint },
  rowText: { flex: 1, minWidth: 0, gap: 3 },
  detail: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
  /** The quiet line under a head's large title. */
  headLine: { fontFamily: font.chrome, fontSize: size.label, lineHeight: 18, color: t.faint },
  segmentOn: { ...surface.chosen },
  readout: {
    height: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 9,
    borderRadius: size.radiusSm,
    ...surface.well,
  },
});
