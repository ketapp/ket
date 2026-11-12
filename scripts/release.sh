#!/usr/bin/env bash
#
# Cuts a release of ket: bumps the version, files the changelog, tags the
# commit, builds the signed and notarised app through scripts/bundle-macos.sh,
# and writes what a release publishes beside it.
#
# Usage:
#   scripts/release.sh <version> [--beta]
#   scripts/release.sh --new-key
#
# Needs KET_SIGN_IDENTITY and KET_NOTARY_PROFILE in the environment, as
# scripts/bundle-macos.sh reads them, and the release key in the login
# keychain. --new-key makes that key once: an Ed25519 key that signs each
# channel manifest, so an installed ket can tell a manifest we wrote from one
# a compromised host serves. Losing it strands every ket installed with its
# public half, so back it up when it is made.
#
# A release, in order:
#
# 1. Refuses unless on main with a clean tree, a version above the current
#    one, fmt and clippy clean, and something under "## Unreleased" in
#    CHANGELOG.md, since that section becomes the release notes.
# 2. Sets the workspace version and the phone app's (mobile/app's app.json,
#    package.json and package-lock.json), files Unreleased under
#    "## <version> — <date>", commits "chore(release): <version>" and tags
#    v<version>. Run
#    again on that tagged commit, after a failed build say, and it goes
#    straight to 3.
# 3. Builds with KET_CHANNEL, KET_BUILD_NUMBER, KET_COMMIT and KET_BUILD_DATE
#    set (see crates/ket-core/src/build_info.rs). The build number is the
#    commit count on main, which only goes up; it becomes CFBundleVersion and
#    is what an installed ket compares to find an update.
# 4. Writes target/release/bundle/release-<version>/: the notarised disk
#    image, a zip of the stapled app for the updater, the release notes, and
#    the channel manifest, stable.json or beta.json.
#
# The manifest is one line of JSON, {"manifest":{...},"signature":"..."}, the
# signature being Ed25519 over the exact bytes of the manifest object. Its
# download URLs are ketapp.dev's /releases/v<version>/, which the site
# redirects to the GitHub release, so the files can move without a new build.
#
# Publishes nothing: the end of the run prints the commands that do.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

KEY_SERVICE="ket-release-ed25519"
SITE="https://ketapp.dev"

die() {
    echo "$*" >&2
    exit 1
}

# Ed25519 signing needs OpenSSL 3 or later; /usr/bin/openssl is LibreSSL.
find_openssl() {
    local candidate
    for candidate in "$(command -v openssl || true)" \
        /opt/homebrew/opt/openssl/bin/openssl /usr/local/opt/openssl/bin/openssl \
        /opt/homebrew/opt/openssl@3/bin/openssl /usr/local/opt/openssl@3/bin/openssl; do
        if [[ -x "${candidate}" && "$("${candidate}" version)" =~ ^OpenSSL\ ([3-9]|[1-9][0-9]) ]]; then
            echo "${candidate}"
            return
        fi
    done
    die "signing the manifest needs OpenSSL 3 or later: brew install openssl"
}

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
chmod 700 "${WORK}"

# The release key, out of the keychain into ${WORK} for as long as the run.
KEY="${WORK}/release.der"
load_key() {
    security find-generic-password -s "${KEY_SERVICE}" -w 2>/dev/null | base64 -D >"${KEY}" ||
        die "no release key in the keychain; scripts/release.sh --new-key makes one"
}

public_key() {
    "${OPENSSL}" pkey -inform DER -in "${KEY}" -pubout -outform DER | tail -c 32 | base64
}

OPENSSL="$(find_openssl)"

if [[ "${1:-}" == --new-key ]]; then
    if security find-generic-password -s "${KEY_SERVICE}" >/dev/null 2>&1; then
        die "the keychain already holds a release key (${KEY_SERVICE}); replacing it" \
            "would strand every ket installed with its public half"
    fi
    "${OPENSSL}" genpkey -algorithm ed25519 -outform DER -out "${KEY}"
    # Through security's own prompt rather than its arguments, which any
    # process can read while it runs.
    printf 'add-generic-password -s %s -a ket -l "ket release signing key" -w %s\n' \
        "${KEY_SERVICE}" "$(base64 <"${KEY}")" | security -i >/dev/null
    security find-generic-password -s "${KEY_SERVICE}" >/dev/null 2>&1 ||
        die "could not put the release key in the keychain"
    echo "made the release key and put it in the login keychain as ${KEY_SERVICE}"
    echo "  public key: $(public_key)"
    echo "  back it up now: Keychain Access, then the item ${KEY_SERVICE}"
    exit 0
fi

VERSION="${1:-}"
[[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    die "usage: scripts/release.sh <major.minor.patch> [--beta] | --new-key"
CHANNEL=stable
case "${2:-}" in
    "") ;;
    --beta) CHANNEL=beta ;;
    *) die "unknown argument: $2" ;;
esac

[[ -n "${KET_SIGN_IDENTITY:-}" && "${KET_SIGN_IDENTITY}" != - ]] ||
    die "KET_SIGN_IDENTITY must name the Developer ID Application identity"
[[ -n "${KET_NOTARY_PROFILE:-}" ]] ||
    die "KET_NOTARY_PROFILE must name the notarytool keychain profile"
load_key

TAG="v${VERSION}"
[[ "$(git symbolic-ref --short HEAD 2>/dev/null)" == main ]] || die "releases are cut from main"
[[ -z "$(git status --porcelain)" ]] || die "the tree has changes; commit or stash them first"

if [[ "$(git describe --exact-match --tags HEAD 2>/dev/null)" == "${TAG}" ]]; then
    echo "HEAD is already ${TAG}; building it again"
else
    git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null &&
        die "${TAG} already tags another commit"
    CURRENT="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
    [[ "${CURRENT}" != "${VERSION}" &&
        "$(printf '%s\n%s\n' "${CURRENT}" "${VERSION}" | sort -V | tail -1)" == "${VERSION}" ]] ||
        die "${VERSION} is not above the current version, ${CURRENT}"
    awk '/^## Unreleased$/ { on = 1; next } on && /^## / { exit } on && /^- / { found = 1 }
        END { exit !found }' CHANGELOG.md ||
        die "CHANGELOG.md has nothing under ## Unreleased to release"

    echo "checking formatting and lints"
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

    echo "releasing ${VERSION} (${CHANNEL})"
    # The first version line is [workspace.package]'s; every crate inherits it.
    awk -v v="${VERSION}" '!done && /^version = "/ { print "version = \"" v "\""; done = 1; next } { print }' \
        Cargo.toml >"${WORK}/Cargo.toml"
    cp "${WORK}/Cargo.toml" Cargo.toml
    cargo metadata --format-version 1 --offline >/dev/null
    # The phone app carries ket's version too, so a phone and the Mac it
    # pairs with name the same release. Its build number and version code
    # stay its own: they count store uploads, not releases.
    python3 - "${VERSION}" <<'PY'
import json
import sys

version = sys.argv[1]


def bump(path, *keys):
    with open(path, encoding="utf-8") as file:
        data = json.load(file)
    for key in keys:
        node = data
        for part in key[:-1]:
            node = node[part]
        node[key[-1]] = version
    with open(path, "w", encoding="utf-8") as file:
        file.write(json.dumps(data, indent=2, ensure_ascii=False) + "\n")


bump("mobile/app/app.json", ("expo", "version"))
bump("mobile/app/package.json", ("version",))
bump("mobile/app/package-lock.json", ("version",), ("packages", "", "version"))
PY
    awk -v head="## ${VERSION} — $(date +%F)" '/^## Unreleased$/ { print; print ""; print head; next } { print }' \
        CHANGELOG.md >"${WORK}/CHANGELOG.md"
    cp "${WORK}/CHANGELOG.md" CHANGELOG.md
    git add Cargo.toml Cargo.lock CHANGELOG.md \
        mobile/app/app.json mobile/app/package.json mobile/app/package-lock.json
    git commit -q -m "chore(release): ${VERSION}"
    git tag -a "${TAG}" -m "ket ${VERSION}"
fi

export KET_CHANNEL="${CHANNEL}"
KET_BUILD_NUMBER="$(git rev-list --count HEAD)"
KET_COMMIT="$(git rev-parse --short=7 HEAD)"
KET_BUILD_DATE="$(git log -1 --format=%cs HEAD)"
export KET_BUILD_NUMBER KET_COMMIT KET_BUILD_DATE
scripts/bundle-macos.sh

BUNDLE="target/release/bundle"
OUT="${BUNDLE}/release-${VERSION}"
DMG="ket-${VERSION}.dmg"
ZIP="ket-${VERSION}.zip"
xcrun stapler validate -q "${BUNDLE}/ket.app" || die "${BUNDLE}/ket.app has no notarisation ticket"
rm -rf "${OUT}"
mkdir -p "${OUT}"
cp "${BUNDLE}/${DMG}" "${OUT}/"
ditto -c -k --sequesterRsrc --keepParent "${BUNDLE}/ket.app" "${OUT}/${ZIP}"

# This version's section of the changelog, without its heading.
awk -v head="## ${VERSION} — " 'index($0, head) == 1 { on = 1; next } on && /^## / { exit } on { print }' \
    CHANGELOG.md | sed -e '/./,$!d' >"${OUT}/notes.md"
[[ -s "${OUT}/notes.md" ]] || die "CHANGELOG.md has no section for ${VERSION}"

PROTOCOL="$(sed -n 's/^pub const PROTOCOL: u32 = \([0-9]*\);/\1/p' crates/ket-core/src/host/wire.rs)"
[[ -n "${PROTOCOL}" ]] || die "could not read PROTOCOL from crates/ket-core/src/host/wire.rs"
MINIMUM="$(plutil -extract LSMinimumSystemVersion raw -o - "${BUNDLE}/ket.app/Contents/Info.plist")"

M_VERSION="${VERSION}" M_BUILD="${KET_BUILD_NUMBER}" M_DATE="${KET_BUILD_DATE}" \
    M_MINIMUM="${MINIMUM}" M_PROTOCOL="${PROTOCOL}" \
    M_ZIP="${SITE}/releases/${TAG}/${ZIP}" M_DMG="${SITE}/releases/${TAG}/${DMG}" \
    M_SHA256="$(shasum -a 256 "${OUT}/${ZIP}" | cut -d ' ' -f 1)" \
    M_SIZE="$(stat -f %z "${OUT}/${ZIP}")" M_NOTES="${OUT}/notes.md" \
    python3 -I -c '
import json, os, sys
e = os.environ
manifest = {
    "version": e["M_VERSION"], "build": int(e["M_BUILD"]), "date": e["M_DATE"],
    "minimum_macos": e["M_MINIMUM"], "protocol": int(e["M_PROTOCOL"]),
    "zip": e["M_ZIP"], "dmg": e["M_DMG"], "sha256": e["M_SHA256"], "size": int(e["M_SIZE"]),
    "notes": open(e["M_NOTES"], encoding="utf-8").read().strip(),
}
sys.stdout.write(json.dumps(manifest, ensure_ascii=False, separators=(",", ":")))
' >"${WORK}/manifest.json"

"${OPENSSL}" pkeyutl -sign -rawin -inkey "${KEY}" -keyform DER \
    -in "${WORK}/manifest.json" -out "${WORK}/manifest.sig"
# Checked against the public half before anything is written, so a manifest
# that would fail an installed ket's check is never published.
"${OPENSSL}" pkey -inform DER -in "${KEY}" -pubout -out "${WORK}/release.pub"
"${OPENSSL}" pkeyutl -verify -rawin -pubin -inkey "${WORK}/release.pub" \
    -in "${WORK}/manifest.json" -sigfile "${WORK}/manifest.sig" >/dev/null ||
    die "the manifest's signature does not verify"
printf '{"manifest":%s,"signature":"%s"}\n' "$(cat "${WORK}/manifest.json")" \
    "$(base64 <"${WORK}/manifest.sig")" >"${OUT}/${CHANNEL}.json"

PRERELEASE=""
if [[ "${CHANNEL}" == beta ]]; then
    PRERELEASE=" --prerelease"
fi
echo
echo "released ${VERSION} (${CHANNEL}, build ${KET_BUILD_NUMBER}, ${KET_COMMIT}) in ${OUT}:"
echo "  ${DMG}  sha256 $(shasum -a 256 "${OUT}/${DMG}" | cut -d ' ' -f 1)"
echo "  ${ZIP}  sha256 $(shasum -a 256 "${OUT}/${ZIP}" | cut -d ' ' -f 1)"
echo "  ${CHANNEL}.json, signed by $(public_key)"
echo "  notes.md"
echo
echo "to publish it:"
echo "  1. push main and ${TAG} to GitHub"
echo "  2. gh release create ${TAG} --title 'ket ${VERSION}' --notes-file ${OUT}/notes.md${PRERELEASE} \\"
echo "       ${OUT}/${DMG} ${OUT}/${ZIP} ${OUT}/${CHANNEL}.json"
echo "  3. set the Homebrew cask to ${VERSION} with the disk image's sha256"
