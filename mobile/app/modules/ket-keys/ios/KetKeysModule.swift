// Keyboard shortcuts for an iPad with a hardware keyboard. React Native's
// text fields report a key but not the modifiers held with it, so ⌘Return
// is caught here instead: UIKit asks every responder from the focused field
// up for its key commands, and this view sits around the field.

import ExpoModulesCore
import UIKit

public class KetKeysModule: Module {
  public func definition() -> ModuleDefinition {
    Name("KetKeys")

    View(KeyCommandsView.self) {
      Events("onSend")

      // What holding ⌘ lists the shortcut as.
      Prop("title") { (view: KeyCommandsView, title: String) in
        view.title = title
      }
    }
  }
}

class KeyCommandsView: ExpoView {
  let onSend = EventDispatcher()
  var title = "Send"

  override var keyCommands: [UIKeyCommand]? {
    let send = UIKeyCommand(title: title, action: #selector(sendPressed), input: "\r", modifierFlags: .command)
    // Ahead of the text field, which would otherwise take Return as a new line.
    send.wantsPriorityOverSystemBehavior = true
    return [send]
  }

  @objc func sendPressed() {
    onSend([:])
  }
}
