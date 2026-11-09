#!/usr/bin/env bash
#
# Runs the shell from a release build.
#
# "Release" rather than "prod" because that is what the profile is called —
# cargo's word, and the one scripts/bundle-macos.sh already uses. Same thing.
#
# The first build after a change to ket-ui takes minutes, not seconds: it is
# the crate with gpui in it. Worth it for anything about how the app *feels* —
# scrolling, the terminal grid, a diff of any size — all of which are
# misleading in a debug build. For everything else, ket-dev.sh.
#
# Usage:
#   ./ket-prod.sh [-- <args passed to ket-ui>]
#   KET_CHROMIUM=0 ./ket-prod.sh [-- <args passed to ket-ui>]
#
# Browser tabs are embedded Chromium, which only runs from an app bundle, so
# this builds target/release/ket-dev.app around ket and runs it from there;
# see scripts/chromium-dev.sh. KET_CHROMIUM=0 builds ket without Chromium,
# its browser tabs WebKit, and runs the bare binary as cargo does.
#
# This runs against the **real** data directory — your projects, your
# worktrees. That is the point of running the app; AGENTS.md tells agents
# to use a throwaway XDG_DATA_HOME instead.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

if [[ "${KET_CHROMIUM:-1}" == 0 ]]; then
    exec cargo run --release -p ket-ui --no-default-features "$@"
fi

exec scripts/chromium-dev.sh release "$@"
