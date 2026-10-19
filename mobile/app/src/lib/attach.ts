// Photos and files for a message: picked on the phone and made ready to
// send. A photo is shrunk to what an agent reads and saved as a JPEG, which
// every agent takes as an image; any other file goes as it is, and the Mac
// names it by path in the message. Held as bytes until Send takes them —
// see `HostConnection.sendFiles`.

import * as Clipboard from 'expo-clipboard';
import * as DocumentPicker from 'expo-document-picker';
import { ImageManipulator, SaveFormat } from 'expo-image-manipulator';
import * as ImagePicker from 'expo-image-picker';

import { readFile } from './read-file';
import { Refused } from './refused';

export type Attachment = {
  key: number;
  /** What the Mac keeps it as: "IMG_2041.jpg". */
  name: string;
  /** Where it is drawn from: the shrunk photo, or the picked file. */
  uri: string;
  /** Whether the agent is handed it as an image of its own. */
  image: boolean;
  bytes: Uint8Array;
};

/** The most files one message carries, as the Mac takes them. */
export const MAX_FILES = 10;

/** The most one message carries, every file together: the protocol's 16 MB,
 * less room for the rest of it. */
export const MAX_BYTES = 15 * 1024 * 1024;

/** The longest side a photo is sent at, past which agents shrink it anyway. */
const LONGEST = 2048;

/** What a photo is saved at: small, and still sharp enough to read a
 * screenshot's text. */
const QUALITY = 0.85;

/** Image types an agent reads as they are. The rest — HEIC, TIFF — are
 * made JPEGs. */
const AS_THEY_ARE = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];

let made = 0;

/** `name` with its extension made `.jpg`, or `fallback` when it has none. */
function jpeg(name: string | null | undefined, fallback: string): string {
  const stem = (name ?? '').replace(/\.[^.]*$/, '').trim();
  return `${stem || fallback}.jpg`;
}

/** A photo, at most [`LONGEST`] on its longest side, as a JPEG. */
async function photo(uri: string, name: string): Promise<Attachment> {
  let image = await ImageManipulator.manipulate(uri).renderAsync();
  if (Math.max(image.width, image.height) > LONGEST) {
    const wide = image.width >= image.height;
    const smaller = await ImageManipulator.manipulate(image)
      .resize(wide ? { width: LONGEST } : { height: LONGEST })
      .renderAsync();
    image.release();
    image = smaller;
  }
  const saved = await image.saveAsync({ format: SaveFormat.JPEG, compress: QUALITY });
  image.release();
  return { key: ++made, name, uri: saved.uri, image: true, bytes: await readFile(saved.uri) };
}

/** Screenshots and pictures from the phone's library, as many as `room`. */
export async function fromPhotos(room: number): Promise<Attachment[]> {
  const picked = await ImagePicker.launchImageLibraryAsync({
    mediaTypes: ['images'],
    allowsMultipleSelection: room > 1,
    selectionLimit: room,
    quality: 1,
  });
  if (picked.canceled) return [];
  return Promise.all(picked.assets.slice(0, room).map((asset) => photo(asset.uri, jpeg(asset.fileName, 'photo'))));
}

/** A photo taken now, of a whiteboard or a screen. */
export async function fromCamera(): Promise<Attachment[]> {
  const allowed = await ImagePicker.requestCameraPermissionsAsync();
  if (!allowed.granted) {
    throw new Refused('ket can’t use the camera: it is turned off for ket. Turn it on in Settings, then try again.');
  }
  const taken = await ImagePicker.launchCameraAsync({ mediaTypes: ['images'], quality: 1 });
  if (taken.canceled) return [];
  return Promise.all(taken.assets.map((asset) => photo(asset.uri, jpeg(asset.fileName, 'photo'))));
}

/** Files from Files, iCloud Drive or another app, as many as `room`. An
 * image an agent cannot read as it is becomes a JPEG. */
export async function fromFiles(room: number): Promise<Attachment[]> {
  const picked = await DocumentPicker.getDocumentAsync({ multiple: room > 1, copyToCacheDirectory: true });
  if (picked.canceled) return [];
  return Promise.all(
    picked.assets.slice(0, room).map(async (asset) => {
      const type = asset.mimeType ?? '';
      if (type.startsWith('image/') && type !== 'image/svg+xml' && !AS_THEY_ARE.includes(type)) {
        return photo(asset.uri, jpeg(asset.name, 'image'));
      }
      return {
        key: ++made,
        name: asset.name,
        uri: asset.uri,
        image: AS_THEY_ARE.includes(type) || type === 'image/svg+xml',
        bytes: await readFile(asset.uri),
      };
    }),
  );
}

/** Whether there is a copied image to paste. */
export function canPaste(): Promise<boolean> {
  return Clipboard.hasImageAsync().catch(() => false);
}

/** The image copied on the phone. */
export async function fromClipboard(): Promise<Attachment[]> {
  const copied = await Clipboard.getImageAsync({ format: 'png' });
  if (!copied) return [];
  return [await photo(copied.data, 'pasted.jpg')];
}

/** How big a file is, for its tile: "14 KB", "2.1 MB". */
export function bytesLabel(count: number): string {
  if (count < 1024) return `${count} B`;
  if (count < 1024 * 1024) return `${Math.round(count / 1024)} KB`;
  return `${(count / (1024 * 1024)).toFixed(1)} MB`;
}

/** The words the Mac adds under a message for the files it could not hand
 * the agent as images — see `ket_core::attachments::with_files` — split off,
 * so a message read back shows them as files rather than as paths. */
export function splitFiles(text: string): { words: string; files: string[] } {
  const at = text.lastIndexOf('Attached files:\n');
  if (at < 0 || (at > 0 && !text.slice(0, at).endsWith('\n\n'))) return { words: text, files: [] };
  const lines = text.slice(at + 'Attached files:\n'.length).split('\n');
  if (lines.length === 0 || !lines.every((line) => line.startsWith('- '))) return { words: text, files: [] };
  return { words: text.slice(0, at).trimEnd(), files: lines.map((line) => line.slice(2)) };
}

/** A path's last part: what a file is called. */
export function fileName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}
