// The chat's mic: what is said, transcribed as it is spoken and streamed into
// the draft. iOS gives an app no way to start the keyboard's dictation, so
// this is a recogniser of the app's own — modules/ket-voice, Apple's Speech
// framework, on the iPhone wherever it can. The browser build's is
// voice.web.ts.

import { NativeModule, requireOptionalNativeModule } from 'expo';
import { Platform } from 'react-native';

import { Refused } from './refused';

type Events = {
  onHeard(event: { text: string; final: boolean }): void;
  onLevel(event: { level: number }): void;
  onEnd(event: { error: string }): void;
};

declare class KetVoice extends NativeModule<Events> {
  authorize(): Promise<'granted' | 'speech' | 'microphone'>;
  start(onDeviceOnly: boolean): Promise<boolean>;
  stop(): void;
  abandon(): void;
}

/** Missing from a build made before the module was, and off iOS: the mic is
 * then not drawn at all. */
const native: KetVoice | null = Platform.OS === 'ios' ? requireOptionalNativeModule<KetVoice>('KetVoice') : null;

/** Whether this build can take dictation. */
export const canDictate = native !== null;

export interface Listening {
  /** Whether what is said stays on the phone. */
  onDevice: boolean;
  /** Closes the microphone; the last words follow, then `onEnd`. */
  stop(): void;
  /** Closes it and drops what has not been transcribed. */
  abandon(): void;
}

export interface Ears {
  /** Everything said since listening began, as the recogniser now has it. */
  onHeard(text: string): void;
  /** How loud the microphone is, 0 to 1. */
  onLevel(level: number): void;
  /** Finished, with why when nothing was heard. */
  onEnd(error?: string): void;
}

/** Opens the microphone, asking for it the first time. `onDeviceOnly` is for
 * a message to a local agent, whose words must not leave the phone. Throws
 * with words for the person; a `Refused` when a permission is off. */
export async function listen(onDeviceOnly: boolean, ears: Ears): Promise<Listening> {
  if (!native) throw new Error('This build of ket can’t take dictation. Use the keyboard’s mic instead.');
  const access = await native.authorize();
  if (access !== 'granted') {
    throw new Refused('ket can’t hear you: Microphone or Speech Recognition is turned off for ket. Turn both on in Settings, then tap the mic again.');
  }
  const subscriptions: { remove(): void }[] = [];
  const done = () => {
    for (const subscription of subscriptions) subscription.remove();
    subscriptions.length = 0;
  };
  subscriptions.push(
    native.addListener('onHeard', ({ text }) => ears.onHeard(text)),
    native.addListener('onLevel', ({ level }) => ears.onLevel(level)),
    native.addListener('onEnd', ({ error }) => {
      done();
      ears.onEnd(error || undefined);
    }),
  );
  let onDevice: boolean;
  try {
    onDevice = await native.start(onDeviceOnly);
  } catch (e) {
    done();
    throw e instanceof Error ? new Error(e.message) : e;
  }
  return {
    onDevice,
    stop: () => native.stop(),
    abandon: () => {
      done();
      native.abandon();
    },
  };
}
