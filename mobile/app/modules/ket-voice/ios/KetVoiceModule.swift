// Dictation for the chat's mic, as a module the app can call: see
// Listener.swift for the recogniser, and src/lib/voice.ts for the app's side.

import ExpoModulesCore

public class KetVoiceModule: Module {
  private let listener = Listener()

  public func definition() -> ModuleDefinition {
    Name("KetVoice")

    Events("onHeard", "onLevel", "onEnd")

    OnCreate {
      self.listener.onHeard = { [weak self] text, final in
        self?.sendEvent("onHeard", ["text": text, "final": final])
      }
      self.listener.onLevel = { [weak self] level in
        self?.sendEvent("onLevel", ["level": level])
      }
      self.listener.onEnd = { [weak self] error in
        self?.sendEvent("onEnd", ["error": error ?? ""])
      }
    }

    // "granted", or the permission refused: "speech" or "microphone".
    AsyncFunction("authorize") { (promise: Promise) in
      Listener.authorize { access in
        promise.resolve(access.rawValue)
      }
    }

    // Whether what is said is transcribed on the phone.
    AsyncFunction("start") { (onDeviceOnly: Bool, promise: Promise) in
      DispatchQueue.main.async {
        do {
          promise.resolve(try self.listener.start(onDeviceOnly: onDeviceOnly))
        } catch let unheard as Unheard {
          promise.reject("ERR_KET_VOICE", unheard.message)
        } catch {
          promise.reject("ERR_KET_VOICE", error.localizedDescription)
        }
      }
    }

    Function("stop") {
      DispatchQueue.main.async { self.listener.stop() }
    }

    Function("abandon") {
      DispatchQueue.main.async { self.listener.abandon() }
    }

    OnDestroy {
      self.listener.abandon()
    }
  }
}
