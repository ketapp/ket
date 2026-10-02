#!/usr/bin/env bash
#
# Assembles ket.app — the installable macOS application.
#
# Hand-rolled rather than reaching for cargo-bundle: a .app is a directory with
# a plist in it, and owning fifty lines of shell is cheaper than owning a build
# dependency that wants its own opinions about metadata.
#
# Usage:
#   scripts/bundle-macos.sh [--debug] [--binary <name>]
#
# Produces target/<profile>/bundle/ket.app, ad-hoc signed so it launches on this
# machine. Handing it to another Mac additionally needs a Developer ID
# certificate and notarisation, which is deliberately not done here: it costs an
# Apple Developer account, and a personal tool does not need one.

set -euo pipefail

PROFILE="release"
CARGO_FLAGS=("--release")
# Overridable so the bundling can be exercised before the UI compiles.
PACKAGE="ket-ui"
# The file cargo actually produces, which is the *binary* name rather than the
# package name — they differ for ket-cli, whose binary is `ket`.
BIN_FILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            PROFILE="debug"
            CARGO_FLAGS=()
            shift
            ;;
        --binary)
            PACKAGE="$2"
            shift 2
            ;;
        --bin-file)
            BIN_FILE="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="target/${PROFILE}/bundle/ket.app"

BIN_FILE="${BIN_FILE:-${PACKAGE}}"

echo "building ${PACKAGE} (${PROFILE})"
cargo build -p "${PACKAGE}" "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}"

echo "assembling ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"

cp "target/${PROFILE}/${BIN_FILE}" "${APP}/Contents/MacOS/ket"

# LSMinimumSystemVersion is 11.0 because gpui draws through Metal and targets
# Big Sur upward; claiming anything lower would let it install and then fail.
cat > "${APP}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>ket</string>
    <key>CFBundleDisplayName</key>
    <string>ket</string>
    <key>CFBundleIdentifier</key>
    <string>dev.ketapp.ket</string>
    <key>CFBundleExecutable</key>
    <string>ket</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleIconFile</key>
    <string>ket.icns</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <!-- Developer previews commonly run on plain-HTTP localhost. This relaxes
         transport security only for local networking, not arbitrary hosts. -->
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
    <!-- Voice input in the quick prompt. macOS quotes these when it asks, and
         kills a process that asks without them. -->
    <key>NSMicrophoneUsageDescription</key>
    <string>ket listens while you dictate a prompt.</string>
    <key>NSSpeechRecognitionUsageDescription</key>
    <string>ket turns what you dictate into the text of a prompt.</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

if [[ -f "assets/ket.icns" ]]; then
    cp "assets/ket.icns" "${APP}/Contents/Resources/ket.icns"
else
    echo "note: assets/ket.icns is missing; the app will use the generic icon"
fi

# Ad-hoc signature. Enough for the app to launch locally, and explicitly not a
# distribution signature — see the header.
codesign --force --deep --sign - "${APP}" 2>/dev/null

echo
echo "built ${APP}"
echo "  open ${APP}"
echo "  cp -R ${APP} /Applications/"
