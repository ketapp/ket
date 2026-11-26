// Puts the code chips in place as the app starts, before any text is drawn.
// See CodeChips.swift.

import ExpoModulesCore

public class CodeChipsSubscriber: ExpoAppDelegateSubscriber {
  @objc public func subscriberDidRegister() {
    CodeChips.install()
  }
}
