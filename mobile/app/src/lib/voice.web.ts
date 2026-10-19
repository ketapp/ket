// The chat's mic in a browser: its own speech recognition — see voice.ts.
// Safari's sends the audio to Apple to be transcribed, so a message bound for
// a local agent, whose words are not to leave your machines, is refused.

/** What a browser's recogniser looks like, as far as this uses it. */
interface Recognition {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onresult: ((event: { results: ArrayLike<ArrayLike<{ transcript: string }>> }) => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
  abort(): void;
}

type RecognitionClass = new () => Recognition;

const Speech: RecognitionClass | undefined =
  typeof window === 'undefined'
    ? undefined
    : ((window as unknown as { SpeechRecognition?: RecognitionClass; webkitSpeechRecognition?: RecognitionClass })
        .SpeechRecognition ??
      (window as unknown as { webkitSpeechRecognition?: RecognitionClass }).webkitSpeechRecognition);

export const canDictate = Speech !== undefined;

export interface Listening {
  onDevice: boolean;
  stop(): void;
  abandon(): void;
}

export interface Ears {
  onHeard(text: string): void;
  onLevel(level: number): void;
  onEnd(error?: string): void;
}

/** Why a browser's recogniser stopped, for the person. */
function said(error: string): string {
  switch (error) {
    case 'not-allowed':
    case 'service-not-allowed':
      return 'ket can’t hear you: this browser has the microphone turned off for this page. Allow it in its settings, then tap the mic again.';
    case 'no-speech':
      return 'Nothing was heard. Tap the mic and try again.';
    case 'audio-capture':
      return 'No microphone is available.';
    case 'network':
      return 'The browser couldn’t reach its speech service.';
    default:
      return `Dictation stopped: ${error}.`;
  }
}

export async function listen(onDeviceOnly: boolean, ears: Ears): Promise<Listening> {
  if (!Speech) throw new Error('This browser can’t take dictation. Use the keyboard’s mic instead.');
  if (onDeviceOnly) {
    throw new Error('The browser sends what you say to be transcribed elsewhere, and a local agent keeps your words on your Mac. Type the message instead.');
  }
  const recognition = new Speech();
  recognition.continuous = true;
  recognition.interimResults = true;
  recognition.lang = typeof navigator === 'undefined' ? 'en-US' : navigator.language;
  let heard = '';
  let failed: string | undefined;
  let over = false;
  recognition.onresult = (event) => {
    heard = Array.from(event.results, (result) => result[0]?.transcript ?? '')
      .join('')
      .trim();
    ears.onHeard(heard);
  };
  recognition.onerror = (event) => {
    if (event.error !== 'aborted') failed = said(event.error);
  };
  recognition.onend = () => {
    if (over) return;
    over = true;
    ears.onEnd(heard ? undefined : failed);
  };
  recognition.start();
  return {
    onDevice: false,
    stop: () => recognition.stop(),
    abandon: () => {
      over = true;
      recognition.abort();
    },
  };
}
