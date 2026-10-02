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
#
# This runs against the **real** data directory — your projects, your
# worktrees. That is the point of running the app; AGENTS.md tells agents
# to use a throwaway XDG_DATA_HOME instead.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

exec cargo run --release -p ket-ui "$@"
