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
#
# This runs against the **real** data directory — your projects, your
# worktrees. That is the point of running the app; AGENTS.md tells agents
# to use a throwaway XDG_DATA_HOME instead.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

exec cargo run -p ket-ui "$@"
