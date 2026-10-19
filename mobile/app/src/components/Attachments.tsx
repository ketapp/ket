// Photos and files in the chat: the + menu over the composer, the tray of
// what is attached inside the field, and the photos above a message of
// yours. See lib/attach.ts for picking them.

import { useEffect } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import Svg, { Circle } from 'react-native-svg';

import { type Attachment, bytesLabel } from '../lib/attach';
import { font, size, t, tint } from '../lib/theme';
import { Camera, Clipboard, Close, FileGlyph, Info, Picture } from './icons';
import { Row } from './ui';

export type Source = 'photos' | 'camera' | 'files' | 'paste';

/** The + menu: where a photo or a file comes from. In place of the `/` menu,
 * over the composer. A desktop too old to take files says so instead. */
export function AttachMenu({
  older,
  pasteable,
  onPick,
}: {
  /** The desktop's name, when its ket is too old to take files. */
  older?: string;
  /** Whether a copied image is there to paste. */
  pasteable: boolean;
  onPick: (source: Source) => void;
}) {
  if (older !== undefined) {
    return (
      <View style={styles.menu}>
        <View style={styles.older}>
          <View style={styles.mark}>
            <Info size={17} color={t.dim} weight={2} />
          </View>
          <Text style={styles.olderText}>
            ket on <Text style={{ color: t.text }}>{older}</Text> is older than this app. Update it, rebuild ket and start a
            new host, to send photos and files. Dictation works as it is.
          </Text>
        </View>
      </View>
    );
  }
  return (
    <ScrollView style={styles.menu} contentContainerStyle={styles.menuInner} keyboardShouldPersistTaps="handled">
      <Row
        mark={<Picture size={17} color={t.text} weight={2} />}
        title="Photos"
        detail="Screenshots and pictures on this iPhone"
        onPress={() => onPick('photos')}
      />
      <Row
        mark={<Camera size={17} color={t.text} weight={2} />}
        title="Camera"
        detail="Take a photo, of a whiteboard or a screen"
        onPress={() => onPick('camera')}
      />
      <Row
        mark={<FileGlyph size={17} color={t.text} weight={2} />}
        title="Files"
        detail="From Files, iCloud Drive or another app"
        onPress={() => onPick('files')}
      />
      {pasteable ? (
        <Row
          mark={<Clipboard size={17} color={t.text} weight={2} />}
          title="Paste image"
          detail="The image you copied"
          onPress={() => onPick('paste')}
        />
      ) : null}
    </ScrollView>
  );
}

/** What is attached, inside the field above the words: each with its ×. */
export function Tray({ files, onRemove }: { files: Attachment[]; onRemove: (key: number) => void }) {
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      keyboardShouldPersistTaps="handled"
      accessibilityLabel="Attached"
      style={styles.tray}
      contentContainerStyle={styles.trayInner}
    >
      {files.map((file) => (
        <View key={file.key} style={file.image ? styles.thumbBox : styles.tileBox}>
          {file.image ? (
            <Image accessibilityLabel={file.name} source={{ uri: file.uri }} style={styles.thumb} />
          ) : (
            <View style={styles.tile}>
              <FileGlyph size={16} color={t.dim} weight={2} />
              <View style={styles.tileText}>
                <Text style={styles.tileName} numberOfLines={1}>
                  {file.name}
                </Text>
                <Text style={styles.tileSize}>{bytesLabel(file.bytes.length)}</Text>
              </View>
            </View>
          )}
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`Remove ${file.name}`}
            hitSlop={8}
            onPress={() => onRemove(file.key)}
            style={({ pressed }) => [styles.remove, pressed && { backgroundColor: t.border }]}
          >
            <Close size={10} color={t.text} weight={3.2} />
          </Pressable>
        </View>
      ))}
    </ScrollView>
  );
}

/** The photos and files a message of yours carried, above its words: as the
 * phone has them, so only while it still does. Faded, with a turning ring,
 * while they are on their way to the Mac. */
export function SentFiles({ files, sending }: { files: Attachment[]; sending?: boolean }) {
  const photos = files.filter((file) => file.image);
  const others = files.filter((file) => !file.image);
  return (
    <>
      {photos.length > 0 ? (
        <View style={styles.photos}>
          {photos.map((file) => (
            <View key={file.key} style={styles.photoBox}>
              <Image
                accessibilityLabel={sending ? `${file.name}, sending` : file.name}
                source={{ uri: file.uri }}
                style={styles.photo}
              />
              {sending ? (
                <View style={styles.veil}>
                  <Spinner />
                </View>
              ) : null}
            </View>
          ))}
        </View>
      ) : null}
      {others.map((file) => (
        <FileChip key={file.key} name={file.name} faded={sending} />
      ))}
    </>
  );
}

/** A file a message carried, by name. */
export function FileChip({ name, faded }: { name: string; faded?: boolean }) {
  return (
    <View style={[styles.fileChip, faded && styles.fileChipFaded]}>
      <FileGlyph size={13} color={faded ? tint(t.text, 0.7) : t.dim} weight={2.2} />
      <Text style={[styles.fileChipText, faded && { color: tint(t.text, 0.7) }]} numberOfLines={1}>
        {name}
      </Text>
    </View>
  );
}

/** How many images a message read back carried, inside its bubble: the
 * conversation has their count, not their pictures. */
export function ImagesChip({ count }: { count: number }) {
  return (
    <View style={styles.images}>
      <Picture size={13} color={t.dim} weight={2.2} />
      <Text style={styles.imagesText}>{count === 1 ? '1 image' : `${count} images`}</Text>
    </View>
  );
}

/** A ring turning while a photo is on its way. */
function Spinner() {
  const turn = useSharedValue(0);
  useEffect(() => {
    turn.set(withRepeat(withTiming(360, { duration: 900, easing: Easing.linear }), -1));
  }, [turn]);
  const turning = useAnimatedStyle(() => ({ transform: [{ rotate: `${turn.get()}deg` }] }));
  return (
    <Animated.View style={turning}>
      <Svg width={30} height={30} viewBox="0 0 30 30">
        <Circle cx={15} cy={15} r={12} fill="none" stroke={tint(t.text, 0.25)} strokeWidth={2.5} />
        <Circle
          cx={15}
          cy={15}
          r={12}
          fill="none"
          stroke={t.text}
          strokeWidth={2.5}
          strokeLinecap="round"
          strokeDasharray="24 75.4"
        />
      </Svg>
    </Animated.View>
  );
}

const styles = StyleSheet.create({
  menu: { flexGrow: 0, maxHeight: 248, borderTopWidth: 1, borderTopColor: t.rule, backgroundColor: t.backdrop },
  menuInner: { padding: 6, gap: 2 },
  // A row of the menu's, as `Row` lays one out, with room for its sentence.
  older: { flexDirection: 'row', alignItems: 'center', gap: 11, minHeight: size.row, margin: 6, paddingVertical: 9, paddingLeft: 14, paddingRight: 12 },
  mark: { width: 16, alignItems: 'center' },
  olderText: { flex: 1, minWidth: 0, fontFamily: font.prose, fontSize: size.label, lineHeight: 19, color: t.dim },
  tray: { flexGrow: 0 },
  // Room above for the × badges, which hang over each tile's corner.
  trayInner: { gap: 10, paddingTop: 12, paddingHorizontal: 12, paddingBottom: 2 },
  thumbBox: { width: 64, height: 64 },
  tileBox: { width: 116, height: 64 },
  thumb: { width: 64, height: 64, borderRadius: size.radiusLg, borderWidth: 1, borderColor: t.border, backgroundColor: t.backdrop },
  tile: {
    flex: 1,
    borderRadius: size.radiusLg,
    borderWidth: 1,
    borderColor: t.border,
    backgroundColor: t.card,
    paddingVertical: 9,
    paddingHorizontal: 10,
    justifyContent: 'space-between',
  },
  tileText: { gap: 1, minWidth: 0 },
  tileName: { fontFamily: font.proseMedium, fontSize: size.detail, lineHeight: 16, color: t.text },
  tileSize: { fontFamily: font.chrome, fontSize: size.caption, lineHeight: 14, color: t.dim },
  remove: {
    position: 'absolute',
    top: -6,
    right: -6,
    width: 22,
    height: 22,
    borderRadius: 11,
    borderWidth: 2,
    borderColor: t.sunken,
    backgroundColor: t.selection,
    alignItems: 'center',
    justifyContent: 'center',
  },
  photos: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', gap: 4 },
  photoBox: { width: 96, height: 128 },
  photo: { width: 96, height: 128, borderRadius: 14, borderWidth: 1, borderColor: t.border, backgroundColor: t.card },
  veil: {
    ...StyleSheet.absoluteFill,
    borderRadius: 14,
    backgroundColor: tint(t.backdrop, 0.55),
    alignItems: 'center',
    justifyContent: 'center',
  },
  fileChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 30,
    maxWidth: '100%',
    paddingLeft: 9,
    paddingRight: 11,
    borderRadius: 15,
    backgroundColor: t.selection,
  },
  fileChipFaded: { backgroundColor: tint(t.selection, 0.7) },
  fileChipText: { flexShrink: 1, fontFamily: font.prose, fontSize: size.detail, color: t.text },
  images: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 24,
    paddingLeft: 7,
    paddingRight: 9,
    borderRadius: 6,
    backgroundColor: t.sunken,
  },
  imagesText: { fontFamily: font.prose, fontSize: size.detail, color: t.dim },
});
