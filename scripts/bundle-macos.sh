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
#   KET_CHROMIUM=0 scripts/bundle-macos.sh [--debug]
#
# Produces target/<profile>/bundle/ket.app, ad-hoc signed so it launches on this
# machine. Handing it to another Mac additionally needs a Developer ID
# certificate and notarisation, which is deliberately not done here: it costs an
# Apple Developer account, and a personal tool does not need one.
#
# ket is built with embedded Chromium (ket-ui's default `chromium` feature), and
# the app gets what CEF needs beside the executable: its framework and five
# helper apps in Contents/Frameworks, put there by scripts/chromium-dev.sh
# --embed. That app is signed inside out rather than with --deep, which would
# re-sign each helper without its entitlements. KET_CHROMIUM=0 builds ket
# without Chromium, its browser tabs WebKit, and --binary or --bin-file bundles
# something other than ket, so neither has any of that.

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

CHROMIUM="${KET_CHROMIUM:-1}"
if [[ "${PACKAGE}" != "ket-ui" || -n "${BIN_FILE}" ]]; then
    if [[ "${KET_CHROMIUM:-}" == 1 ]]; then
        echo "KET_CHROMIUM=1 bundles ket-ui; --binary and --bin-file do not apply" >&2
        exit 2
    fi
    CHROMIUM=0
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="target/${PROFILE}/bundle/ket.app"

BIN_FILE="${BIN_FILE:-${PACKAGE}}"
# Where cargo put the binaries. A Chromium build honours CARGO_TARGET_DIR; a
# plain one reads ./target as it always has.
BIN_DIR="target/${PROFILE}"

echo "building ${PACKAGE} (${PROFILE})"
if [[ "${CHROMIUM}" == 1 ]]; then
    # cef-rs downloads the CEF distribution on the first build, a few hundred
    # megabytes once (see scripts/chromium-dev.sh), which copies the framework
    # out of wherever this build found it.
    BIN_DIR="${CARGO_TARGET_DIR:-target}/${PROFILE}"
    cargo build -p ket-ui --features chromium "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}"
    cargo build -p ket-chromium --features cef --bin ket-chromium-helper \
        "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}"
elif [[ "${PACKAGE}" == "ket-ui" ]]; then
    cargo build -p ket-ui --no-default-features "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}"
else
    cargo build -p "${PACKAGE}" "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}"
fi
if [[ "${PACKAGE}" != "ket-cli" ]]; then
    cargo build -p ket-cli "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}"
fi

echo "assembling ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources/bin"

cp "${BIN_DIR}/${BIN_FILE}" "${APP}/Contents/MacOS/ket"
cp "${BIN_DIR}/ket" "${APP}/Contents/Resources/bin/ket"

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

if [[ "${CHROMIUM}" == 1 ]]; then
    # The framework, the helpers and the Info.plist keys CEF wants; signs the
    # framework and each helper as it puts them in.
    scripts/chromium-dev.sh --embed "${APP}" "${BIN_DIR}" dev.ketapp.ket
fi

# Ad-hoc signature. Enough for the app to launch locally, and explicitly not a
# distribution signature — see the header.
if [[ "${CHROMIUM}" == 1 ]]; then
    # Inside out: the framework and helpers are signed already, so the CLI and
    # then the app around them all.
    codesign --force --sign - "${APP}/Contents/Resources/bin/ket" 2>/dev/null
    codesign --force --sign - "${APP}" 2>/dev/null
else
    codesign --force --deep --sign - "${APP}" 2>/dev/null
fi

echo
echo "built ${APP}"
echo "  open ${APP}"
echo "  cp -R ${APP} /Applications/"
