#!/usr/bin/env bash
#
# Runs the shell with embedded Chromium (CEF), from an app bundle.
#
# CEF does not run from a bare binary in target/. It loads its framework from
# ../Frameworks beside the executable, and starts its GPU, renderer and
# utility processes from five helper apps that have to sit in that same
# Frameworks directory, as separate bundles. So this builds ket-ui with
# the `chromium` feature, assembles target/<profile>/ket-dev.app around it and
# runs the executable inside: what `cargo run` does, one bundle deeper.
#
# Usage:
#   scripts/chromium-dev.sh <debug|release> [<cargo build flags>] [-- <args passed to ket-ui>]
#   scripts/chromium-dev.sh --embed <app> <binary dir> <bundle id>
#
# The first builds ket-ui, the helper and the CLI and assembles the app from
# all three, so cargo flags that have to agree between them go to every
# build: --target-dir, --release and --profile, which also say where the app
# is assembled, and --locked, --offline, --frozen, -v, -q, -j and --color. The
# last of <debug|release>, --release and --profile is the profile. Feature
# flags (--features, -F, --all-features, --no-default-features) are ket-ui's
# alone, on top of its `chromium`. Anything else is refused rather than
# passed to one build: -p, --bin or --target would build a piece the app is
# not assembled from, and --config or -Z could move one where this does not
# look.
#
# ./ket-dev.sh and ./ket-prod.sh run the first unless KET_CHROMIUM=0 is set.
# The second is scripts/bundle-macos.sh's: it puts the framework and helpers
# into an assembled ket.app and signs them, leaving the app's own signature
# to the caller.
#
# CEF is not in the repository. On the first build cef-rs downloads the
# version the cef-dll-sys crate names, a few hundred megabytes once, and builds
# against it: into CEF_PATH when that is set, which shares one download
# between checkouts, and otherwise into cargo's build directory, once per
# profile. The framework is copied from wherever that build says it found it,
# so a plain cargo build and this script build against the same CEF and never
# undo each other's. KET_CEF_DIR overrides that with another directory holding
# "Chromium Embedded Framework.framework".
#
# Re-running is quick. Each piece of ket-dev.app (framework, helpers,
# executable, CLI) is copied again only when its source changed, as an APFS
# clone, and only that piece and the app around it are signed again. A stamp
# per piece, in target/<profile>/ket-dev.stamps, records what it came from.
#
# Signing is inside out: the framework's libraries, the framework, each
# helper with its entitlements (scripts/entitlements/), then the app. Never
# --deep, which re-signs every helper with the app's entitlements in place of
# its own. The Alerts helper needs none. ket-dev.app's framework and helpers
# are signed ad hoc without the hardened runtime, so none of it is enforced
# there: only the hardened runtime checks entitlements.
#
# --embed signs as scripts/bundle-macos.sh says, through the variables its
# header describes: KET_SIGN_IDENTITY (unset or "-" is ad hoc) and
# KET_HARDENED_RUNTIME. A real identity always gets the hardened runtime and
# a secure timestamp, which notarisation asks of every piece of code. Ad hoc
# with the hardened runtime is for trying that runtime out before there is a
# certificate, and it needs one entitlement a release does not: library
# validation admits only code signed by the loading process's own team, an
# ad-hoc signature has no team, and so no helper could load the ad-hoc
# framework. In that mode alone every helper also gets
# disable-library-validation.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SELF="${ROOT}/scripts/chromium-dev.sh"
ENTITLEMENTS="${ROOT}/scripts/entitlements"
FRAMEWORK="Chromium Embedded Framework.framework"
# The oldest macOS Chromium runs on, which CEF's own sample apps declare: an
# app with Chromium in it claims no lower, or it would install and not start.
CHROMIUM_MIN_MACOS=13.0
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

# The helpers CEF starts, as "<name suffix>|<identifier suffix>|<entitlements>".
# CEF is pointed at "ket Helper.app" (crates/ket-chromium/src/engine.rs) and
# finds the others beside it by adding their suffix to that name. The
# identifier suffixes are CEF's own (cmake/cef_variables.cmake), the
# entitlements Chromium's (see each file). In practice CEF 156 starts its
# renderer and GPU processes from the plain helper too, so that one carries
# what they need (helper.plist).
HELPERS=(
    "||helper.plist"
    " (Alerts)|.alerts|"
    " (GPU)|.gpu|helper-gpu.plist"
    " (Plugin)|.plugin|helper-plugin.plist"
    " (Renderer)|.renderer|helper-renderer.plist"
)

say() {
    echo "chromium-dev: $*" >&2
}

die() {
    say "$*"
    exit 1
}

usage() {
    echo "usage: scripts/chromium-dev.sh <debug|release> [<cargo build flags>] [-- <args passed to ket-ui>]" >&2
    echo "       scripts/chromium-dev.sh --embed <app> <binary dir> <bundle id>" >&2
    exit 2
}

# Copies a file or directory as an APFS clone, which takes no time and no
# space until one side changes. cp -c already falls back to a real copy across
# volumes; plain cp covers a cp that cannot clone at all.
clone() {
    rm -rf "$2"
    if ! cp -cR "$1" "$2" 2>/dev/null; then
        rm -rf "$2"
        cp -R "$1" "$2"
    fi
}

# How sign() signs: ad hoc, without the hardened runtime, unless --embed is
# told otherwise (embed_signing). LOOSE_LIBRARY_VALIDATION is set for the one
# mode that needs disable-library-validation, and SCRATCH is where the
# entitlements that adds are written.
SIGN_FLAGS=(--sign -)
LOOSE_LIBRARY_VALIDATION=0
SCRATCH=""

# Reads --embed's signing from the environment; see the top.
embed_signing() {
    local identity="${KET_SIGN_IDENTITY:--}"
    SIGN_FLAGS=(--sign "${identity}")
    if [[ "${identity}" != - ]]; then
        SIGN_FLAGS+=(--options runtime --timestamp)
    elif [[ "${KET_HARDENED_RUNTIME:-0}" == 1 ]]; then
        SIGN_FLAGS+=(--options runtime)
        LOOSE_LIBRARY_VALIDATION=1
        SCRATCH="$(mktemp -d)"
        trap 'rm -rf "${SCRATCH}"' EXIT
    fi
}

# codesign --force reports every signature it replaces on stderr, so its
# output is shown only when it fails.
sign() {
    local out
    out="$(codesign --force "${SIGN_FLAGS[@]}" "$@" 2>&1)" || {
        echo "${out}" >&2
        return 1
    }
}

# Prints the entitlements file to sign the executable <name> with: <file>
# (empty for none) as it is, or a copy that adds disable-library-validation
# when LOOSE_LIBRARY_VALIDATION says so. plutil reads dots as a key path, so
# the dots in the key are escaped.
entitlements_for() {
    local copy
    if [[ "${LOOSE_LIBRARY_VALIDATION}" != 1 ]]; then
        echo "$1"
        return
    fi
    copy="${SCRATCH}/$2.plist"
    if [[ -n "$1" ]]; then
        cp "$1" "${copy}"
    else
        plutil -create xml1 "${copy}"
    fi
    plutil -replace 'com\.apple\.security\.cs\.disable-library-validation' -bool YES "${copy}"
    echo "${copy}"
}

# Path, mtime and size: enough to tell a rebuilt file from the one a piece of
# ket-dev.app was made from, without reading it.
fingerprint() {
    stat -L -f '%N %m %z' "$@"
}

# Prints the directory holding the CEF framework that the build in <binary
# dir> compiled against: cef-dll-sys's build script records it, as CEF_DIR,
# wherever it came from.
cef_dir() {
    local dir output="" candidate
    if [[ -n "${KET_CEF_DIR:-}" ]]; then
        dir="${KET_CEF_DIR%/}"
        if [[ "${dir##*/}" == "${FRAMEWORK}" ]]; then
            dir="$(dirname "${dir}")"
        fi
        if [[ ! -d "${dir}/${FRAMEWORK}" ]]; then
            die "KET_CEF_DIR=${KET_CEF_DIR} holds no ${FRAMEWORK}"
        fi
        echo "${dir}"
        return
    fi

    # One build script run per feature set and environment; the newest is the
    # build just made.
    for candidate in "$1"/build/cef-dll-sys-*/output; do
        if [[ -f "${candidate}" && ( -z "${output}" || "${candidate}" -nt "${output}" ) ]]; then
            output="${candidate}"
        fi
    done
    dir=""
    if [[ -n "${output}" ]]; then
        dir="$(sed -n 's/^cargo::metadata=CEF_DIR=//p' "${output}" | tail -1)"
    fi
    if [[ -z "${dir}" || ! -d "${dir}/${FRAMEWORK}" ]]; then
        die "no build under $1 says where its ${FRAMEWORK} is." \
            "Build ket-ui there first, or point KET_CEF_DIR at the directory holding it."
    fi
    echo "${dir}"
}

# Copies CEF's framework from <cef dir> into <app>/Contents/Frameworks and
# signs it.
embed_framework() {
    local framework="$2/Contents/Frameworks/${FRAMEWORK}" path
    say "copying ${FRAMEWORK} from $1"
    mkdir -p "$2/Contents/Frameworks"
    clone "$1/${FRAMEWORK}" "${framework}"

    # Chromium's own strings (context menus, error pages, dialogs) come in
    # 220 .lproj folders, about 50 MB; ket's UI is English, so the browser's
    # is too. en_* keeps en_GB and the gendered variants of both (en_FEMININE
    # and so on), which Chromium may load in their place.
    # Everything else stays, libvk_swiftshader.dylib included: it is the
    # software renderer the GPU process falls back to.
    for path in "${framework}"/Resources/*.lproj; do
        case "${path##*/}" in
            en.lproj | en_*.lproj) ;;
            *) rm -rf "${path}" ;;
        esac
    done

    # A bundle's signature records the signatures of the code inside it and
    # refuses code that has none, so the framework's libraries go first.
    for path in "${framework}"/Libraries/* "${framework}"/Helpers/*; do
        if [[ -f "${path}" && "$(file -b "${path}")" == Mach-O* ]]; then
            sign "${path}"
        fi
    done
    sign "${framework}"
}

# Info.plist for one helper: <name> <identifier>.
helper_plist() {
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$1</string>
    <key>CFBundleDisplayName</key>
    <string>$1</string>
    <key>CFBundleIdentifier</key>
    <string>$2</string>
    <key>CFBundleExecutable</key>
    <string>$1</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>${CHROMIUM_MIN_MACOS}</string>
    <!-- Keeps the helpers out of the Dock and the app switcher. -->
    <key>LSUIElement</key>
    <true/>
    <!-- Lets the GPU process stay on the integrated GPU of a two-GPU Mac
         rather than waking the discrete one. -->
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <!-- Chromium's own helpers set this (chrome/app/helper-Info.plist). -->
    <key>LSEnvironment</key>
    <dict>
        <key>MallocNanoZone</key>
        <string>0</string>
    </dict>
</dict>
</plist>
PLIST
}

# Builds the five helper apps in <app>/Contents/Frameworks from
# <binary dir>/ket-chromium-helper, identified below <bundle id>, and signs
# each with its entitlements.
embed_helpers() {
    local binary="$2/ket-chromium-helper" spec suffix id_suffix entitlements name helper
    if [[ ! -f "${binary}" ]]; then
        die "no ${binary}; cargo build -p ket-chromium --features cef --bin ket-chromium-helper"
    fi
    for spec in "${HELPERS[@]}"; do
        IFS='|' read -r suffix id_suffix entitlements <<<"${spec}"
        name="ket Helper${suffix}"
        helper="$1/Contents/Frameworks/${name}.app"
        rm -rf "${helper}"
        mkdir -p "${helper}/Contents/MacOS"
        clone "${binary}" "${helper}/Contents/MacOS/${name}"
        helper_plist "${name}" "$3.helper${id_suffix}" >"${helper}/Contents/Info.plist"
        entitlements="$(entitlements_for "${entitlements:+${ENTITLEMENTS}/${entitlements}}" "${name}")"
        if [[ -n "${entitlements}" ]]; then
            sign --entitlements "${entitlements}" "${helper}"
        else
            sign "${helper}"
        fi
    done
}

# What hosting CEF's browser process adds to an app's Info.plist.
# MallocNanoZone=0 is Chromium's own setting for its app
# (chrome/app/app-Info.plist), so CEF runs on the allocator setup Chromium is
# tested with. Launch Services applies it when the app is opened, and
# everything the app starts inherits it, shells included; for them it only
# means malloc skips one fast path. The usage strings are there because macOS
# kills a process that asks for the camera or Bluetooth without one, and any
# web page can ask.
chromium_plist_keys() {
    plutil -replace LSMinimumSystemVersion -string "${CHROMIUM_MIN_MACOS}" "$1"
    plutil -replace LSEnvironment -json '{"MallocNanoZone":"0"}' "$1"
    plutil -replace NSCameraUsageDescription \
        -string "A page open in ket's browser asked to use the camera." "$1"
    plutil -replace NSBluetoothAlwaysUsageDescription \
        -string "A page open in ket's browser asked to use Bluetooth." "$1"
}

# The --embed mode, for scripts/bundle-macos.sh: <app> <binary dir> <bundle id>.
embed() {
    local cef
    if [[ ! -f "$1/Contents/Info.plist" ]]; then
        die "$1 has no Contents/Info.plist to add to"
    fi
    embed_signing
    cef="$(cef_dir "$2")"
    embed_framework "${cef}" "$1"
    embed_helpers "$1" "$2" "$3"
    chromium_plist_keys "$1/Contents/Info.plist"
}

# The dev app's Info.plist: ket.app's (scripts/bundle-macos.sh) under the
# identifier the bare dev binary embeds (crates/ket-ui/build.rs), so a
# development build's permissions stay apart from the installed app's.
dev_plist() {
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>ket</string>
    <key>CFBundleDisplayName</key>
    <string>ket</string>
    <key>CFBundleIdentifier</key>
    <string>dev.ketapp.ket.dev</string>
    <key>CFBundleExecutable</key>
    <string>ket</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleIconFile</key>
    <string>ket.icns</string>
    <key>LSMinimumSystemVersion</key>
    <string>${CHROMIUM_MIN_MACOS}</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
    <key>NSMicrophoneUsageDescription</key>
    <string>ket listens while you dictate a prompt.</string>
    <key>NSSpeechRecognitionUsageDescription</key>
    <string>ket turns what you dictate into the text of a prompt.</string>
</dict>
</plist>
PLIST
}

# Brings one piece of the dev app up to date: runs the rest of the arguments
# as a command unless <path> exists and stamp <name> already records
# <fingerprint>. The stamp is cleared first and written only once the command
# succeeds, so an interrupted run redoes the piece.
refresh() {
    local name="$1" fingerprint="$2" path="$3"
    shift 3
    if [[ -e "${path}" && "$(cat "${STAMPS}/${name}" 2>/dev/null)" == "${fingerprint}" ]]; then
        return 0
    fi
    rm -f "${STAMPS}/${name}"
    "$@"
    echo "${fingerprint}" >"${STAMPS}/${name}"
}

# Clones the CLI into <app>/Contents/Resources/bin and signs it.
embed_cli() {
    mkdir -p "$2/Contents/Resources/bin"
    clone "$1" "$2/Contents/Resources/bin/ket"
    sign "$2/Contents/Resources/bin/ket"
}

# The dev mode: <debug|release> [<cargo build flags>] [-- <args passed to ket-ui>].
# Which flags go where is in the usage at the top.
run_dev() {
    local profile="$1" target_dir="" shared=() features=() app_args=()
    local flag value valued
    shift
    case "${profile}" in
        debug | release) ;;
        *) usage ;;
    esac
    # Split the way `cargo run` does: flags for cargo, then -- and the app's.
    while [[ $# -gt 0 ]]; do
        flag="$1" value="" valued=false
        shift
        case "${flag}" in
            --)
                app_args=("$@")
                break
                ;;
            --*=*) value="${flag#*=}" flag="${flag%%=*}" valued=true ;;
            -F?* | -j?*) value="${flag:2}" flag="${flag:0:2}" valued=true ;;
        esac
        case "${flag}" in
            --profile | --target-dir | --features | -F | -j | --jobs | --color)
                if ! ${valued} && [[ $# -gt 0 ]]; then
                    value="$1"
                    shift
                fi
                if [[ -z "${value}" ]]; then
                    die "${flag} needs a value"
                fi
                ;;
            --release | --locked | --offline | --frozen | -v | -vv | --verbose | -q | --quiet | \
                --all-features | --no-default-features)
                if ${valued}; then
                    die "${flag} takes no value"
                fi
                ;;
            *)
                die "${flag} cannot be passed on: the app is assembled from ket-ui," \
                    "the helper and the CLI, which have to be built alike. It takes" \
                    "--target-dir, --release, --profile, --locked, --offline, --frozen," \
                    "-v, -q, -j and --color, and --features for ket-ui alone; see the" \
                    "top of scripts/chromium-dev.sh."
                ;;
        esac
        case "${flag}" in
            --release) profile=release ;;
            --profile) profile="${value}" ;;
            --target-dir) target_dir="${value}" ;;
            --features | -F) features+=(--features "${value}") ;;
            --all-features | --no-default-features) features+=("${flag}") ;;
            -j | --jobs | --color) shared+=("${flag}" "${value}") ;;
            *) shared+=("${flag}") ;;
        esac
    done

    # The profile's flag, and the directory cargo names after it.
    local build=() dir
    case "${profile}" in
        debug | dev) dir=debug ;;
        release) build=(--release) dir=release ;;
        test) build=(--profile test) dir=debug ;;
        bench) build=(--profile bench) dir=release ;;
        *) build=(--profile "${profile}") dir="${profile}" ;;
    esac

    # Where cargo builds, in the order cargo looks, each relative to where it
    # runs, which is here; every build is told the same.
    local target="${target_dir:-${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-target}}}"
    if [[ "${target}" != /* ]]; then
        target="${ROOT}/${target}"
    fi
    if [[ -n "${target_dir}" ]]; then
        shared+=(--target-dir "${target}")
    fi
    build+=("${shared[@]+"${shared[@]}"}")

    cargo build -p ket-ui --features chromium \
        "${build[@]+"${build[@]}"}" "${features[@]+"${features[@]}"}"
    cargo build -p ket-chromium --features cef --bin ket-chromium-helper \
        "${build[@]+"${build[@]}"}"
    # The CLI too: its terminals' `ket` comes from beside the app's own
    # binary, and one left there by another checkout's build would lack the
    # browser commands this build's tabs answer.
    cargo build -p ket-cli "${build[@]+"${build[@]}"}"

    local bin="${target}/${dir}" file
    local app="${bin}/ket-dev.app"
    for file in ket-ui ket-chromium-helper; do
        if [[ ! -f "${bin}/${file}" ]]; then
            die "cargo built no ${bin}/${file}"
        fi
    done

    STAMPS="${bin}/ket-dev.stamps"
    if [[ ! -d "${app}" ]]; then
        rm -rf "${STAMPS}"
    fi
    mkdir -p "${app}/Contents/MacOS" "${app}/Contents/Resources" "${STAMPS}"

    local cef recipe fp_framework fp_helpers fp_cli fp_icon
    cef="$(cef_dir "${bin}")"
    # A change to this script or to an entitlements file redoes every piece.
    recipe="$(fingerprint "${SELF}" "${ENTITLEMENTS}"/*.plist)"

    fp_framework="${recipe} $(fingerprint "${cef}/${FRAMEWORK}/Chromium Embedded Framework")"
    refresh framework "${fp_framework}" "${app}/Contents/Frameworks/${FRAMEWORK}" \
        embed_framework "${cef}" "${app}"

    fp_helpers="${recipe} $(fingerprint "${bin}/ket-chromium-helper")"
    refresh helpers "${fp_helpers}" "${app}/Contents/Frameworks/ket Helper (Renderer).app" \
        embed_helpers "${app}" "${bin}" dev.ketapp.ket.dev

    # The executable is a link to the one cargo built, not a copy. The ket
    # host takes a connection for a ket window only when its program is the
    # host's own file (same_program in crates/ket-core/src/host/server.rs):
    # a host already running from target/<profile>/ket-ui would take a copy
    # for an outsider, with no hook reports, no ket start and no pairing.
    # What reads the executable's path, CEF finding its framework and ket
    # its CLI, reads the link's, inside the app.
    ln -sfn "${bin}/ket-ui" "${app}/Contents/MacOS/ket"

    # From a bundle, ket-ui puts the CLI on its terminals' PATH from
    # Contents/Resources/bin rather than from beside itself (cli_path_for in
    # crates/ket-ui/src/terminal.rs). Copied when one has been built, which is
    # when a bare dev binary would find one too.
    if [[ -f "${bin}/ket" ]]; then
        fp_cli="$(fingerprint "${bin}/ket")"
        refresh cli "${fp_cli}" "${app}/Contents/Resources/bin/ket" \
            embed_cli "${bin}/ket" "${app}"
    fi

    if [[ -f "${ROOT}/assets/ket.icns" ]]; then
        fp_icon="$(fingerprint "${ROOT}/assets/ket.icns")"
        refresh icon "${fp_icon}" "${app}/Contents/Resources/ket.icns" \
            clone "${ROOT}/assets/ket.icns" "${app}/Contents/Resources/ket.icns"
    fi

    # Built beside the stamps and moved in only when it differs, so an
    # unchanged plist keeps its modification time.
    dev_plist >"${STAMPS}/Info.plist"
    chromium_plist_keys "${STAMPS}/Info.plist"
    if ! cmp -s "${STAMPS}/Info.plist" "${app}/Contents/Info.plist"; then
        mv "${STAMPS}/Info.plist" "${app}/Contents/Info.plist"
    fi

    # The app itself stays unsigned: codesign will not seal a bundle whose
    # executable is a link, and nothing checks the seal of an app run by
    # exec. The framework and helpers, which Chromium starts, are signed.
    rm -rf "${app}/Contents/_CodeSignature"

    # Launch Services applies the plist's LSEnvironment when an app is
    # opened; exec does not go through it.
    export MallocNanoZone=0
    say "running ${app}"
    exec "${app}/Contents/MacOS/ket" "${app_args[@]+"${app_args[@]}"}"
}

case "${1:-}" in
    --embed)
        if [[ $# -ne 4 ]]; then
            usage
        fi
        embed "$2" "$3" "$4"
        ;;
    debug | release)
        run_dev "$@"
        ;;
    *)
        usage
        ;;
esac
