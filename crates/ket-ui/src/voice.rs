//! Voice input for the quick prompt: the microphone, transcribed as it is
//! spoken by Apple's Speech framework.
//!
//! On the device where the Mac supports it, which every Apple-silicon Mac
//! does for the common languages — so nothing that is said leaves the
//! machine. Where it does not, the framework falls back to Apple's servers,
//! the same as the system's own Dictation.
//!
//! # Why this is `unsafe`
//!
//! The third of the workspace's documented carve-outs (see its lint block).
//! Speech and AVAudioEngine are Objective-C frameworks with no safe binding:
//! every message `objc2` generates for them is an `unsafe fn`, because the
//! bindings cannot prove what AppKit does with the arguments. What is done
//! with them here is the documented, ordinary use — a tap on the input node
//! appending buffers to a recognition request — and nothing reaches past it.
//!
//! # The two permissions
//!
//! Recording needs the microphone, and transcribing needs Speech Recognition;
//! macOS asks for each once. Both prompts quote a usage description from the
//! app's `Info.plist`, and a process that asks without one is killed on the
//! spot rather than refused — which is why [`authorize`] looks for them first.
//! `ket.app` carries them in its bundle, and `build.rs` embeds the same keys
//! in the bare binary so `cargo run` can ask too.
//!
//! # Threads
//!
//! Neither framework calls back on the main thread. The recogniser's results
//! land in a shared [`Heard`] that the dialog reads on its own schedule, and
//! the permission answers come back through one-shot channels.

use std::sync::{Arc, Mutex};

/// What the recogniser has made of the audio so far.
#[derive(Debug, Default)]
pub(crate) struct Heard {
    /// The best transcription of everything said since listening began.
    /// Rewritten wholesale on every result, since a later word can change
    /// the recogniser's mind about an earlier one.
    pub(crate) text: String,
    /// Whether the recogniser is finished: a final result, or an error.
    pub(crate) done: bool,
    /// Why it stopped early, when it did.
    pub(crate) error: Option<String>,
}

/// The shared transcript a [`Dictation`] writes and the dialog reads.
pub(crate) type Transcript = Arc<Mutex<Heard>>;

#[cfg(target_os = "macos")]
pub(crate) use mac::{Dictation, authorize};

#[cfg(not(target_os = "macos"))]
pub(crate) use elsewhere::{Dictation, authorize};

#[cfg(target_os = "macos")]
mod mac {
    use std::ptr::NonNull;
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_avf_audio::{
        AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioEngine, AVAudioInputNode,
        AVAudioPCMBuffer, AVAudioTime,
    };
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_speech::{
        SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
        SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
    };
    use tokio::sync::oneshot;

    use super::{Heard, Transcript};

    /// The input bus the tap sits on. An input node has exactly one.
    const BUS: usize = 0;

    /// Frames per tap callback: about 20ms at 48kHz, which is how often the
    /// recogniser is handed new audio.
    const TAP_FRAMES: u32 = 1024;

    /// Asks for everything listening needs, prompting the first time.
    ///
    /// Speech Recognition first, then the microphone, so the second prompt
    /// only appears once the first was answered yes. Refused either way is
    /// an error worded for the person, naming where to change their mind.
    pub(crate) async fn authorize() -> Result<(), String> {
        usage_descriptions()?;
        speech_access().await?;
        microphone_access().await
    }

    /// Whether the running binary can ask at all — see the module doc.
    fn usage_descriptions() -> Result<(), String> {
        let bundle = NSBundle::mainBundle();
        let has = |key: &str| {
            bundle
                .objectForInfoDictionaryKey(&NSString::from_str(key))
                .is_some()
        };
        if has("NSMicrophoneUsageDescription") && has("NSSpeechRecognitionUsageDescription") {
            Ok(())
        } else {
            Err("This build of ket can't ask for the microphone. Run ket.app instead.".into())
        }
    }

    async fn speech_access() -> Result<(), String> {
        let refused = || {
            "Speech Recognition is off for ket. Turn it on in System Settings › Privacy & Security."
                .to_owned()
        };
        // SAFETY: a class method with no arguments.
        let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
        match status {
            SFSpeechRecognizerAuthorizationStatus::Authorized => return Ok(()),
            SFSpeechRecognizerAuthorizationStatus::NotDetermined => {}
            _ => return Err(refused()),
        }

        let (tell, answer) = oneshot::channel();
        let tell = Mutex::new(Some(tell));
        let handler = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
            if let Some(tell) = tell.lock().ok().and_then(|mut tell| tell.take()) {
                let _ = tell.send(status == SFSpeechRecognizerAuthorizationStatus::Authorized);
            }
        });
        // SAFETY: the block is 'static and only sends on a channel; the usage
        // description the prompt quotes was checked for above.
        unsafe { SFSpeechRecognizer::requestAuthorization(&handler) };
        match answer.await {
            Ok(true) => Ok(()),
            _ => Err(refused()),
        }
    }

    async fn microphone_access() -> Result<(), String> {
        let refused = || {
            "ket can't use the microphone. Allow it in System Settings › Privacy & Security."
                .to_owned()
        };
        // Before macOS 14 there is no way to ask ahead of time: starting the
        // engine asks instead, and records silence until it is answered.
        if !objc2::available!(macos = 14.0) {
            return Ok(());
        }
        // SAFETY: the shared instance is a process-wide singleton.
        let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
        match permission {
            AVAudioApplicationRecordPermission::Granted => return Ok(()),
            AVAudioApplicationRecordPermission::Undetermined => {}
            _ => return Err(refused()),
        }

        let (tell, answer) = oneshot::channel();
        let tell = Mutex::new(Some(tell));
        let handler = RcBlock::new(move |granted: Bool| {
            if let Some(tell) = tell.lock().ok().and_then(|mut tell| tell.take()) {
                let _ = tell.send(granted.as_bool());
            }
        });
        // SAFETY: as for `speech_access`.
        unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&handler) };
        match answer.await {
            Ok(true) => Ok(()),
            _ => Err(refused()),
        }
    }

    /// The microphone, open and being transcribed into a [`Transcript`].
    ///
    /// Stops listening when dropped, and abandons the transcription with it:
    /// a dialog that closes mid-sentence does not want the rest.
    pub(crate) struct Dictation {
        engine: Retained<AVAudioEngine>,
        input: Retained<AVAudioInputNode>,
        request: Retained<SFSpeechAudioBufferRecognitionRequest>,
        task: Retained<SFSpeechRecognitionTask>,
        /// Owned for as long as the task runs; the task does not keep it.
        _recognizer: Retained<SFSpeechRecognizer>,
        listening: bool,
    }

    impl Dictation {
        /// Opens the microphone and starts transcribing into `heard`.
        ///
        /// [`authorize`] first: this assumes both permissions are granted.
        pub(crate) fn start(heard: Transcript) -> Result<Self, String> {
            // SAFETY: every call below is a plain message to an object this
            // function created and owns, in the order Apple's documentation
            // for live transcription gives; the blocks are 'static and touch
            // only what they captured.
            unsafe {
                let recognizer = SFSpeechRecognizer::init(SFSpeechRecognizer::alloc())
                    .ok_or("Speech recognition doesn't support your language settings.")?;
                if !recognizer.isAvailable() {
                    return Err("Speech recognition isn't available right now.".into());
                }

                let request = SFSpeechAudioBufferRecognitionRequest::new();
                request.setShouldReportPartialResults(true);
                if recognizer.supportsOnDeviceRecognition() {
                    request.setRequiresOnDeviceRecognition(true);
                }
                if objc2::available!(macos = 13.0) {
                    request.setAddsPunctuation(true);
                }

                let engine = AVAudioEngine::new();
                let input = engine.inputNode();
                let format = input.outputFormatForBus(BUS);
                if format.sampleRate() <= 0.0 || format.channelCount() == 0 {
                    return Err("No microphone is connected.".into());
                }

                let feed = request.clone();
                let tap = RcBlock::new(
                    move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                        feed.appendAudioPCMBuffer(buffer.as_ref());
                    },
                );
                // The node copies the block, so `tap` need not outlive this.
                input.installTapOnBus_bufferSize_format_block(
                    BUS,
                    TAP_FRAMES,
                    Some(&format),
                    RcBlock::as_ptr(&tap),
                );

                let results = RcBlock::new(
                    move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                        let Ok(mut heard) = heard.lock() else {
                            return;
                        };
                        record(&mut heard, result.as_ref(), error.as_ref());
                    },
                );
                let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &results);

                engine.prepare();
                if let Err(error) = engine.startAndReturnError() {
                    input.removeTapOnBus(BUS);
                    task.cancel();
                    return Err(format!(
                        "Couldn't start the microphone: {}",
                        error.localizedDescription()
                    ));
                }

                Ok(Self {
                    engine,
                    input,
                    request,
                    task,
                    _recognizer: recognizer,
                    listening: true,
                })
            }
        }

        /// Closes the microphone. What was said so far is still transcribed,
        /// and the final result arrives in the [`Transcript`] shortly after.
        pub(crate) fn stop(&mut self) {
            if !self.listening {
                return;
            }
            self.listening = false;
            // SAFETY: messages to objects this owns, undoing what `start` did.
            unsafe {
                self.engine.stop();
                self.input.removeTapOnBus(BUS);
                self.request.endAudio();
            }
        }
    }

    impl Drop for Dictation {
        fn drop(&mut self) {
            self.stop();
            // SAFETY: cancelling a task this owns; a finished one ignores it.
            unsafe { self.task.cancel() };
        }
    }

    /// Writes one callback's worth of news into the transcript.
    ///
    /// # Safety
    ///
    /// `result` and `error` are the recogniser's own arguments, borrowed for
    /// the length of the callback.
    unsafe fn record(
        heard: &mut Heard,
        result: Option<&SFSpeechRecognitionResult>,
        error: Option<&NSError>,
    ) {
        if let Some(result) = result {
            // SAFETY: see the function's contract.
            unsafe {
                heard.text = result.bestTranscription().formattedString().to_string();
                if result.isFinal() {
                    heard.done = true;
                }
            }
        }
        if let Some(error) = error {
            heard.done = true;
            // Once words were heard they are the result, and an error after
            // them — stopping mid-word ends in one — is not news. With none,
            // the error is the only account of what happened.
            if heard.text.is_empty() {
                heard.error = Some(error.localizedDescription().to_string());
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod elsewhere {
    use super::Transcript;

    const UNSUPPORTED: &str = "Voice input is only available on macOS for now.";

    pub(crate) async fn authorize() -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }

    pub(crate) struct Dictation;

    impl Dictation {
        pub(crate) fn start(_heard: Transcript) -> Result<Self, String> {
            Err(UNSUPPORTED.into())
        }

        pub(crate) fn stop(&mut self) {}
    }
}
