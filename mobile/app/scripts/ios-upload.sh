#!/usr/bin/env bash
#
# Builds the phone app for the App Store and uploads it to App Store Connect,
# where it appears in TestFlight once Apple has processed it. Locally, as the
# app is always built: no EAS.
#
# Usage, from mobile/app:
#   KET_TEAM_ID=<team id> ASC_KEY_ID=<key id> ASC_ISSUER_ID=<issuer id> \
#       scripts/ios-upload.sh
#
# KET_TEAM_ID is the Apple developer team that signs the app. ASC_KEY_ID and
# ASC_ISSUER_ID name an App Store Connect API key (Users and Access ›
# Integrations › App Store Connect API), whose .p8 is read from ASC_KEY_PATH,
# by default where Apple's tools look for it:
# ~/.appstoreconnect/private_keys/AuthKey_<key id>.p8. The key signs in for
# both the signing and the upload, so nothing here needs Xcode's account
# settings; with Admin access it also lets Xcode make the distribution
# certificate and profile the first time.
#
# The version and build are app.json's. App Store Connect refuses a build
# number it has seen for that version, so raise ios.buildNumber before each
# upload. Each run keeps dist/ios/<version>-<build>/: the archive, and the
# JavaScript bundle's source map, which is what turns the stack in an error
# report someone shares from Settings › About › Error reports back into files
# and lines. Keep it as long as that build is in use.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

die() {
    echo "$*" >&2
    exit 1
}

[[ -n "${KET_TEAM_ID:-}" ]] || die "KET_TEAM_ID must name the Apple developer team"
[[ -n "${ASC_KEY_ID:-}" ]] || die "ASC_KEY_ID must name the App Store Connect API key"
[[ -n "${ASC_ISSUER_ID:-}" ]] || die "ASC_ISSUER_ID must name the API key's issuer"
KEY="${ASC_KEY_PATH:-${HOME}/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID}.p8}"
[[ -f "${KEY}" ]] || die "no API key at ${KEY}; set ASC_KEY_PATH"
command -v node >/dev/null || die "node must be on PATH: Expo and the bundle step run it"
command -v pod >/dev/null || die "CocoaPods must be installed: brew install cocoapods"

read -r VERSION BUILD < <(node -e '
  const app = require("./app.json").expo;
  console.log(app.version, app.ios.buildNumber);
')
OUT="dist/ios/${VERSION}-${BUILD}"
[[ ! -e "${OUT}" ]] || die "${OUT} exists: build ${BUILD} of ${VERSION} was made already; raise ios.buildNumber in app.json"
mkdir -p "${OUT}"
OUT="$(cd "${OUT}" && pwd)"

AUTH=(
    -allowProvisioningUpdates
    -authenticationKeyPath "${KEY}"
    -authenticationKeyID "${ASC_KEY_ID}"
    -authenticationKeyIssuerID "${ASC_ISSUER_ID}"
)

echo "generating the iOS project for ${VERSION} (${BUILD})"
npx expo prebuild --platform ios --clean --no-install
(cd ios && pod install)

echo "archiving"
SOURCEMAP_FILE="${OUT}/main.jsbundle.map" xcodebuild \
    -workspace ios/ket.xcworkspace -scheme ket -configuration Release \
    -destination 'generic/platform=iOS' -archivePath "${OUT}/ket.xcarchive" \
    DEVELOPMENT_TEAM="${KET_TEAM_ID}" CODE_SIGN_STYLE=Automatic \
    "${AUTH[@]}" archive

cat >"${OUT}/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key><string>app-store-connect</string>
  <key>destination</key><string>upload</string>
  <key>teamID</key><string>${KET_TEAM_ID}</string>
  <key>signingStyle</key><string>automatic</string>
  <key>uploadSymbols</key><true/>
  <key>manageAppVersionAndBuildNumber</key><false/>
</dict>
</plist>
PLIST

echo "uploading to App Store Connect"
xcodebuild -exportArchive -archivePath "${OUT}/ket.xcarchive" \
    -exportOptionsPlist "${OUT}/ExportOptions.plist" -exportPath "${OUT}/export" \
    "${AUTH[@]}"

echo
echo "uploaded ${VERSION} (${BUILD}). App Store Connect processes it for a few"
echo "minutes, then it is in TestFlight. Kept in ${OUT}:"
echo "  ket.xcarchive, main.jsbundle.map"
