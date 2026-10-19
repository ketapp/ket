// The microphone, transcribed as it is spoken by Apple's Speech framework:
// the phone's half of the desktop's crates/ket-ui/src/voice.rs, for the mic
// in the chat's composer. iOS gives an app no way to start the keyboard's
// own dictation, so that mic needs a recogniser of its own.
//
// On the device where the iPhone supports it, which recent iPhones do for
// the common languages — so what is said stays on the phone. Where it does
// not, the framework falls back to Apple's servers, as the keyboard's
// dictation does — except for a message bound for a local agent, which is
// refused instead (see `start`).
//
// No Expo here, so this file type-checks against the iOS SDK alone; the
// module around it is KetVoiceModule.swift.

import AVFoundation
import Foundation
import Speech

/// Why listening could not start, worded for the person.
struct Unheard: Error {
  let message: String
}

/// Which permission is missing, if one is.
enum Access: String {
  case granted
  case speech
  case microphone
}

final class Listener {
  /// The best transcription of everything said since listening began, and
  /// whether it is the last. Rewritten wholesale on every result, since a
  /// later word can change the recogniser's mind about an earlier one.
  var onHeard: ((String, Bool) -> Void)?
  /// How loud the microphone is, 0 to 1, a dozen times a second.
  var onLevel: ((Double) -> Void)?
  /// Finished: a final result, or an error — the error's words only when
  /// nothing was heard, since a stop mid-word ends in one that is not news.
  var onEnd: ((String?) -> Void)?

  private var engine: AVAudioEngine?
  private var request: SFSpeechAudioBufferRecognitionRequest?
  private var task: SFSpeechRecognitionTask?
  private var recognizer: SFSpeechRecognizer?
  /// Which run a callback belongs to: one from a run already abandoned is
  /// dropped rather than written over the next.
  private var run = 0
  private var heardAny = false
  private var ended = true
  private var lastLevel = Date.distantPast

  /// The bus the tap sits on. An input node has exactly one.
  private let bus: AVAudioNodeBus = 0
  /// Frames per tap callback: about 20ms at 48kHz.
  private let tapFrames: AVAudioFrameCount = 1024

  /// Asks for everything listening needs, prompting the first time: Speech
  /// Recognition first, then the microphone, so the second prompt only
  /// appears once the first was answered yes.
  static func authorize(_ done: @escaping (Access) -> Void) {
    SFSpeechRecognizer.requestAuthorization { status in
      guard status == .authorized else {
        done(.speech)
        return
      }
      let answered: (Bool) -> Void = { granted in done(granted ? .granted : .microphone) }
      if #available(iOS 17.0, *) {
        AVAudioApplication.requestRecordPermission(completionHandler: answered)
      } else {
        AVAudioSession.sharedInstance().requestRecordPermission(answered)
      }
    }
  }

  /// Opens the microphone and starts transcribing. `onDeviceOnly` is for a
  /// message bound for a local agent: there the words must not leave the
  /// phone either, so a language it cannot transcribe itself is refused
  /// rather than sent to Apple. Returns whether it is transcribed here.
  ///
  /// `authorize` first: this assumes both permissions are granted. On the
  /// main thread.
  func start(onDeviceOnly: Bool) throws -> Bool {
    abandon()
    guard let recognizer = SFSpeechRecognizer() else {
      throw Unheard(message: "Speech recognition doesn’t support your language settings.")
    }
    guard recognizer.isAvailable else {
      throw Unheard(message: "Speech recognition isn’t available right now.")
    }
    let request = SFSpeechAudioBufferRecognitionRequest()
    request.shouldReportPartialResults = true
    request.addsPunctuation = true
    let onDevice = recognizer.supportsOnDeviceRecognition
    if onDevice {
      request.requiresOnDeviceRecognition = true
    } else if onDeviceOnly {
      throw Unheard(
        message:
          "This iPhone can’t transcribe your language on its own, and a local agent keeps your words on your Mac. Type the message instead."
      )
    }

    let session = AVAudioSession.sharedInstance()
    do {
      try session.setCategory(.record, mode: .measurement, options: .duckOthers)
      try session.setActive(true, options: .notifyOthersOnDeactivation)
    } catch {
      throw Unheard(message: "Couldn’t open the microphone: \(error.localizedDescription)")
    }

    let engine = AVAudioEngine()
    let input = engine.inputNode
    let format = input.outputFormat(forBus: bus)
    guard format.sampleRate > 0, format.channelCount > 0 else {
      try? session.setActive(false, options: .notifyOthersOnDeactivation)
      throw Unheard(message: "No microphone is available.")
    }

    run += 1
    let current = run
    heardAny = false
    ended = false
    input.installTap(onBus: bus, bufferSize: tapFrames, format: format) { [weak self] buffer, _ in
      request.append(buffer)
      self?.measure(buffer)
    }
    let task = recognizer.recognitionTask(with: request) { [weak self] result, error in
      let text = result?.bestTranscription.formattedString
      let final = result?.isFinal ?? false
      let why = error?.localizedDescription
      DispatchQueue.main.async {
        self?.heard(run: current, text: text, final: final, error: why)
      }
    }

    engine.prepare()
    do {
      try engine.start()
    } catch {
      input.removeTap(onBus: bus)
      task.cancel()
      try? session.setActive(false, options: .notifyOthersOnDeactivation)
      ended = true
      throw Unheard(message: "Couldn’t start the microphone: \(error.localizedDescription)")
    }
    self.engine = engine
    self.request = request
    self.task = task
    self.recognizer = recognizer
    return onDevice
  }

  /// Closes the microphone. What was said so far is still transcribed, and
  /// the final result follows shortly after.
  func stop() {
    guard let engine else { return }
    engine.stop()
    engine.inputNode.removeTap(onBus: bus)
    request?.endAudio()
    self.engine = nil
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
  }

  /// Stops listening and drops what has not been transcribed yet: the chat
  /// went away mid-sentence and does not want the rest.
  func abandon() {
    stop()
    task?.cancel()
    task = nil
    request = nil
    recognizer = nil
    run += 1
    ended = true
  }

  private func heard(run: Int, text: String?, final: Bool, error: String?) {
    guard run == self.run, !ended else { return }
    if let text {
      heardAny = heardAny || !text.isEmpty
      onHeard?(text, final)
    }
    guard final || error != nil else { return }
    ended = true
    stop()
    task = nil
    request = nil
    recognizer = nil
    onEnd?(heardAny ? nil : error)
  }

  /// The loudness of one buffer, sent on now and then. On the tap's thread.
  private func measure(_ buffer: AVAudioPCMBuffer) {
    let now = Date()
    guard now.timeIntervalSince(lastLevel) > 0.08, let samples = buffer.floatChannelData?[0] else {
      return
    }
    lastLevel = now
    let count = Int(buffer.frameLength)
    guard count > 0 else { return }
    var sum: Float = 0
    for at in 0..<count {
      sum += samples[at] * samples[at]
    }
    let rms = (sum / Float(count)).squareRoot()
    // -50 dB is silence and -10 dB a raised voice, on the meter's 0 to 1.
    let decibels = 20 * log10(max(Double(rms), 1e-6))
    let level = min(max((decibels + 50) / 40, 0), 1)
    DispatchQueue.main.async { [weak self] in
      self?.onLevel?(level)
    }
  }
}
