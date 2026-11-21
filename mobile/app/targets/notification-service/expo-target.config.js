// The notification service extension, made into an Xcode target by
// @bacons/apple-targets at prebuild: it opens the notices a paired Mac seals
// to this phone and shows their words — see NotificationService.swift.

/** @type {import('@bacons/apple-targets/app.plugin').ConfigFunction} */
module.exports = (config) => ({
  type: 'notification-service',
  name: 'NotificationService',
  bundleIdentifier: '.NotificationService',
  // CryptoKit's HPKE.
  deploymentTarget: '17.0',
  // The app's own Keychain group, where it keeps the push key.
  entitlements: {
    'keychain-access-groups': config.ios.entitlements['keychain-access-groups'],
  },
});
