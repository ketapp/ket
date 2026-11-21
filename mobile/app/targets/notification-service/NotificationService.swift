// ket's notification service extension: opens a notice a paired Mac sealed
// to this phone's push key (crates/ket-core/src/host/phones/push.rs) and
// shows its real words. Apple and ket's push service carry it sealed under a
// generic line, "An agent has an update", which is what stays when it can't
// be opened here: no key, or a key made since the Mac was told.
//
// The key is the one the app keeps through expo-secure-store as
// `ket.push.key` (src/lib/store.ts). The extension shares the app's Keychain
// group, and the item is readable from the first unlock after a restart, so
// a locked phone opens notices too.
//
// What a tap needs goes in `userInfo["body"]`, where expo-notifications
// reads a remote notification's data: the Mac's payload without its words
// (`pushedOf` in src/lib/push.ts).

import CryptoKit
import Foundation
import Security
import UserNotifications

/// Binds a sealed notice to this use: push.rs's `INFO`.
private let sealedFor = Data("ket push v1".utf8)

/// The length of HPKE's encapsulated key for X25519, which leads the sealed bytes.
private let encapsulatedLength = 32

class NotificationService: UNNotificationServiceExtension {
  private var deliver: ((UNNotificationContent) -> Void)?
  private var generic: UNNotificationContent?

  override func didReceive(
    _ request: UNNotificationRequest,
    withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    deliver = contentHandler
    generic = request.content
    guard let content = request.content.mutableCopy() as? UNMutableNotificationContent,
      let notice = open(request.content.userInfo)
    else {
      contentHandler(request.content)
      return
    }
    if let title = notice["title"] as? String, !title.isEmpty { content.title = title }
    if let body = notice["body"] as? String, !body.isEmpty { content.body = body }
    if let host = notice["host"] as? String, let name = desktopName(host) { content.subtitle = name }
    var data = notice
    data.removeValue(forKey: "title")
    data.removeValue(forKey: "body")
    var userInfo = content.userInfo
    userInfo.removeValue(forKey: "e")
    userInfo["body"] = data
    content.userInfo = userInfo
    contentHandler(content)
  }

  /// Out of time before it was opened: the generic line goes as it came.
  override func serviceExtensionTimeWillExpire() {
    if let deliver, let generic { deliver(generic) }
  }
}

/// The notice sealed in `userInfo["e"]`, opened: HPKE base mode with
/// DHKEM(X25519, HKDF-SHA256), HKDF-SHA256 and ChaCha20-Poly1305, no
/// associated data. Nil when there is nothing to open or it won't open.
private func open(_ userInfo: [AnyHashable: Any]) -> [String: Any]? {
  guard let encoded = userInfo["e"] as? String,
    let sealed = Data(base64Encoded: encoded),
    sealed.count > encapsulatedLength,
    let secret = keychainString("ket.push.key").flatMap(base64URL),
    let key = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: secret)
  else { return nil }
  do {
    var recipient = try HPKE.Recipient(
      privateKey: key,
      ciphersuite: .Curve25519_SHA256_ChachaPoly,
      info: sealedFor,
      encapsulatedKey: Data(sealed.prefix(encapsulatedLength))
    )
    let plain = try recipient.open(Data(sealed.dropFirst(encapsulatedLength)))
    guard let notice = try JSONSerialization.jsonObject(with: plain) as? [String: Any],
      notice["v"] as? Int == 1
    else { return nil }
    return notice
  } catch {
    return nil
  }
}

/// The paired desktop's name, as the app's own notices name it: only when
/// more than one is paired.
private func desktopName(_ host: String) -> String? {
  guard let ids = keychainJSON("ket.hosts") as? [String], ids.count > 1,
    let saved = keychainJSON("ket.host.\(host)") as? [String: Any],
    let name = saved["name"] as? String, !name.isEmpty
  else { return nil }
  return name
}

private func keychainJSON(_ name: String) -> Any? {
  keychainString(name).flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) }
}

/// An entry the app keeps through expo-secure-store: a generic password
/// under its service, with the entry's name as the account, in bytes. Its
/// own service first, then the one entries were written under before it
/// marked them as needing no authentication.
private func keychainString(_ name: String) -> String? {
  for service in ["app:no-auth", "app"] {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: Data(name.utf8),
      kSecMatchLimit as String: kSecMatchLimitOne,
      kSecReturnData as String: true,
    ]
    var item: CFTypeRef?
    if SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
      let data = item as? Data,
      let text = String(data: data, encoding: .utf8)
    {
      return text
    }
  }
  return nil
}

/// Unpadded base64url, as the app writes keys.
private func base64URL(_ text: String) -> Data? {
  var standard = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
  standard += String(repeating: "=", count: (4 - standard.count % 4) % 4)
  return Data(base64Encoded: standard)
}
