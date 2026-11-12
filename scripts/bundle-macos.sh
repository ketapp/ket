#!/usr/bin/env bash
#
# Assembles ket.app — the installable macOS application — and the disk image
# it is released in.
#
# Hand-rolled rather than reaching for cargo-bundle: a .app is a directory with
# a plist in it, and owning fifty lines of shell is cheaper than owning a build
# dependency that wants its own opinions about metadata. The image is the same
# bargain: hdiutil ships with macOS, and a window background is not worth a
# create-dmg.
#
# Usage:
#   scripts/bundle-macos.sh [--debug] [--binary <name>] [--bin-file <file>]
#   KET_CHROMIUM=0 scripts/bundle-macos.sh [--debug]
#   KET_HARDENED_RUNTIME=1 scripts/bundle-macos.sh [--debug]
#   KET_SIGN_IDENTITY="Developer ID Application: Ketapp LLC (<team id>)" \
#       KET_NOTARY_PROFILE=ket scripts/bundle-macos.sh
#
# Produces target/<profile>/bundle/ket.app and, beside it, ket-<version>.dmg
# (the version is the workspace's, from Cargo.toml): the app and a link to
# /Applications to drag it onto. The Homebrew cask downloads exactly that file
# from the GitHub release v<version>, and pins the SHA-256 printed last.
#
# How it is signed is the environment's to say:
#
# KET_SIGN_IDENTITY is the codesign identity. Unset or "-" signs ad hoc, which
# is enough for the app to launch on the Mac that built it and no other. A
# Developer ID Application certificate in the keychain signs for distribution:
# every piece of code in the app with the hardened runtime and a secure
# timestamp, which notarisation asks of all of it, and then the image.
#
# KET_HARDENED_RUNTIME=1 turns the hardened runtime on for an ad-hoc build, to
# find out before there is a certificate whether ket still works under it: its
# entitlements are the release's, so what they fail to allow fails here too.
# It adds the one entitlement a release does not need,
# disable-library-validation, to the app and to every helper. Library
# validation admits only code signed by the loading process's own team, an
# ad-hoc signature has none, and so nothing could load the ad-hoc CEF
# framework. A Developer ID build always has the hardened runtime; it cannot be
# turned off there.
#
# KET_NOTARY_PROFILE names a notarytool keychain profile, made once with
# `xcrun notarytool store-credentials <name> --apple-id ... --team-id ...`.
# With a Developer ID signature the app is notarised and stapled, then the
# image made around it is notarised and stapled too, so the app opens without
# a network check whether it is run from the image or copied out of it. A
# rejection prints Apple's log and fails. Unset, or with an ad-hoc signature,
# notarisation is skipped and the end of the run says so.
#
# ket is built with embedded Chromium (ket-ui's default `chromium` feature), and
# the app gets what CEF needs beside the executable: its framework and five
# helper apps in Contents/Frameworks, put there by scripts/chromium-dev.sh
# --embed, which signs them as above. The app is signed inside out rather than
# with --deep, which would re-sign each helper without its entitlements. The
# app's own are scripts/entitlements/app.plist, each helper's beside it, and
# each file says why it holds what it does. KET_CHROMIUM=0 builds ket without
# Chromium, its browser tabs WebKit, and --binary or --bin-file bundles
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
BUNDLE="target/${PROFILE}/bundle"
APP="${BUNDLE}/ket.app"
DMG="${BUNDLE}/ket-${VERSION}.dmg"

die() {
    echo "$*" >&2
    exit 1
}

# Settled before anything is built, so a mistake here costs no build.
IDENTITY="${KET_SIGN_IDENTITY:--}"
NOTARY="${KET_NOTARY_PROFILE:-}"
if [[ "${IDENTITY}" == - ]]; then
    HARDENED="${KET_HARDENED_RUNTIME:-0}"
else
    HARDENED="${KET_HARDENED_RUNTIME:-1}"
    if [[ "${HARDENED}" != 1 ]]; then
        die "KET_HARDENED_RUNTIME=${HARDENED} with a Developer ID signature: notarisation" \
            "refuses code without the hardened runtime, so it is always on there"
    fi
fi
if [[ "${HARDENED}" != 0 && "${HARDENED}" != 1 ]]; then
    die "KET_HARDENED_RUNTIME is 0 or 1, not ${HARDENED}"
fi
# scripts/chromium-dev.sh --embed signs the framework and helpers by these.
export KET_SIGN_IDENTITY="${IDENTITY}" KET_HARDENED_RUNTIME="${HARDENED}"

SIGN_FLAGS=(--sign "${IDENTITY}")
if [[ "${HARDENED}" == 1 ]]; then
    SIGN_FLAGS+=(--options runtime)
fi
if [[ "${IDENTITY}" != - ]]; then
    # A secure timestamp: notarisation wants one, and it keeps the signature
    # valid after the certificate expires. An ad-hoc signature cannot have one.
    SIGN_FLAGS+=(--timestamp)
fi

# codesign --force reports every signature it replaces on stderr, so its
# output is shown only when it fails.
sign() {
    local out
    out="$(codesign --force "${SIGN_FLAGS[@]}" "$@" 2>&1)" || {
        echo "${out}" >&2
        return 1
    }
}

# Sends <file> to Apple's notary service, waits for its verdict, and staples
# the ticket to <target>, which is <file> unless that is only a zip carrying
# it. A rejection prints Apple's log, which names each file it refused and why.
notarize() {
    local file="$1" target="${2:-$1}" result id status
    echo "notarising ${target} (usually a few minutes)"
    result="$(xcrun notarytool submit "${file}" --keychain-profile "${NOTARY}" \
        --wait --output-format json)" || true
    id="$(plutil -extract id raw -o - - <<<"${result}" 2>/dev/null)" || id=""
    status="$(plutil -extract status raw -o - - <<<"${result}" 2>/dev/null)" || status=""
    if [[ "${status}" != Accepted ]]; then
        echo "notarisation of ${target} ended ${status:-without a verdict}:" >&2
        echo "${result}" >&2
        if [[ -n "${id}" ]]; then
            xcrun notarytool log "${id}" --keychain-profile "${NOTARY}" >&2 || true
        fi
        exit 1
    fi
    xcrun stapler staple "${target}"
}

NOTARISE=0
if [[ -z "${NOTARY}" ]]; then
    SKIPPED="KET_NOTARY_PROFILE is unset"
elif [[ "${IDENTITY}" == - ]]; then
    SKIPPED="Apple notarises nothing signed ad hoc; KET_NOTARY_PROFILE needs KET_SIGN_IDENTITY"
else
    NOTARISE=1
fi

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
# Inside target/, so staging the image can clone the app rather than copy it.
SCRATCH="$(mktemp -d "${BUNDLE}/.work.XXXXXX")"
trap 'rm -rf "${SCRATCH}"' EXIT

cp "${BIN_DIR}/${BIN_FILE}" "${APP}/Contents/MacOS/ket"
cp "${BIN_DIR}/ket" "${APP}/Contents/Resources/bin/ket"

# CFBundleVersion is the build number scripts/release.sh stamps, which only
# goes up; a build without one repeats the version there.
# LSMinimumSystemVersion is 13.0 because the bundled Chromium framework is built
# for macOS 13 and later (gpui alone would draw on Big Sur); claiming anything
# lower would let it install and then fail to open browser tabs.
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
    <string>${KET_BUILD_NUMBER:-${VERSION}}</string>
    <key>CFBundleIconFile</key>
    <string>ket.icns</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
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

# Inside out: the framework and helpers are signed already, so the CLI and
# then the app around them all. Not --deep, even without Chromium, so the
# CLI is never left to whatever signature the linker gave it.
APP_ENTITLEMENTS="scripts/entitlements/app.plist"
if [[ "${IDENTITY}" == - && "${HARDENED}" == 1 ]]; then
    # The ad-hoc trial run's exception; see the header. plutil reads dots as
    # a key path, so the dots in the key are escaped.
    cp "${APP_ENTITLEMENTS}" "${SCRATCH}/app.plist"
    APP_ENTITLEMENTS="${SCRATCH}/app.plist"
    plutil -replace 'com\.apple\.security\.cs\.disable-library-validation' -bool YES \
        "${APP_ENTITLEMENTS}"
fi
RUNTIME_NOTE=""
if [[ "${HARDENED}" == 1 ]]; then
    RUNTIME_NOTE=", hardened runtime"
fi
echo "signing ${APP} as ${IDENTITY}${RUNTIME_NOTE}"
sign "${APP}/Contents/Resources/bin/ket"
sign --entitlements "${APP_ENTITLEMENTS}" "${APP}"
codesign --verify --deep --strict "${APP}"

# Notarisation refuses any Mach-O without the hardened runtime, wherever it
# sits in the app. Finding one here is quicker than hearing it from Apple.
# codesign's answer is read whole: grep -q would stop reading it early, and
# under pipefail the codesign it cut off would count as the failure.
if [[ "${HARDENED}" == 1 ]]; then
    while IFS= read -r -d '' path; do
        if [[ "$(file -b "${path}")" == Mach-O* &&
            "$(codesign -dv "${path}" 2>&1)" != *"CodeDirectory "*"flags="*"runtime"* ]]; then
            die "${path} is not signed with the hardened runtime"
        fi
    done < <(find "${APP}" -type f -print0)
fi

if [[ "${NOTARISE}" == 1 ]]; then
    # The notary service takes a zip, not a bare .app; the ticket is stapled to
    # the app itself, before the image is made around it.
    ditto -c -k --keepParent "${APP}" "${SCRATCH}/ket.zip"
    notarize "${SCRATCH}/ket.zip" "${APP}"
fi

# The image: ket.app and a link to /Applications, the window people drag one
# onto the other in. HFS+ rather than hdiutil's default, APFS: release images
# have long been HFS+ (create-dmg's default too), so it is the well-trodden
# path through Gatekeeper and stapling. ULMO is LZMA, the smallest
# of hdiutil's formats, which macOS reads from 10.15, below anything ket runs
# on. macOS 27 calls hdiutil create deprecated in favour of `diskutil image`;
# it still works, and it is on every macOS a release might be built on.
echo "imaging ${DMG}"
STAGE="${SCRATCH}/image"
mkdir "${STAGE}"
# A clone where the volume can make one, so staging copies nothing.
cp -cR "${APP}" "${STAGE}/" 2>/dev/null || {
    rm -rf "${STAGE:?}/ket.app"
    cp -R "${APP}" "${STAGE}/"
}
ln -s /Applications "${STAGE}/Applications"
rm -f "${DMG}"
hdiutil create -quiet -volname ket -srcfolder "${STAGE}" -fs HFS+ -format ULMO "${DMG}"
hdiutil verify -quiet "${DMG}"
if [[ "${IDENTITY}" != - ]]; then
    # An image takes no hardened runtime, only the signature and timestamp.
    codesign --force --sign "${IDENTITY}" --timestamp "${DMG}"
fi
if [[ "${NOTARISE}" == 1 ]]; then
    notarize "${DMG}"
    # What Gatekeeper will say on someone else's Mac: "accepted" with
    # "source=Notarized Developer ID", or the reason it would refuse.
    spctl --assess --type execute --verbose=2 "${APP}"
    spctl --assess --type open --context context:primary-signature --verbose=2 "${DMG}"
fi

echo
echo "built ${APP}"
echo "  open ${APP}"
echo "  cp -R ${APP} /Applications/"
echo "built ${DMG}"
if [[ "${NOTARISE}" == 1 ]]; then
    echo "  notarised and stapled; attach it to the GitHub release v${VERSION}"
elif [[ "${IDENTITY}" == - ]]; then
    echo "  signed ad hoc and not notarised (${SKIPPED}): it opens on this Mac only"
else
    echo "  signed but not notarised (${SKIPPED}): Gatekeeper refuses it on other Macs"
fi
echo "  sha256 $(shasum -a 256 "${DMG}" | cut -d ' ' -f 1)"
