#!/usr/bin/env bash
#
# Runs the shell from a debug build.
#
# The everyday one: compiles in seconds rather than minutes, keeps assertions
# and line numbers, and is what a panic should be read from. Use ket-prod.sh to
# see how it actually performs.
#
# Usage:
#   ./ket-dev.sh [-- <args passed to ket-ui>]
#   KET_CHROMIUM=0 ./ket-dev.sh [-- <args passed to ket-ui>]
#
# Browser tabs are embedded Chromium, which only runs from an app bundle, so
# this builds target/debug/ket-dev.app around ket and runs it from there;
# see scripts/chromium-dev.sh. KET_CHROMIUM=0 builds ket without Chromium,
# its browser tabs WebKit, and runs the bare binary as cargo does.
#
# This runs against the **real** data directory — your projects, your
# worktrees. That is the point of running the app; AGENTS.md tells agents
# to use a throwaway XDG_DATA_HOME instead.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

if [[ "${KET_CHROMIUM:-1}" == 0 ]]; then
    exec cargo run -p ket-ui --no-default-features "$@"
fi

exec scripts/chromium-dev.sh debug "$@"
