#!/usr/bin/env bash
# Test coverage for the Rust workspace and the phone's two packages, as HTML
# under coverage/ at the repository root:
#
#   coverage/rust/html/index.html     every crate, from cargo test
#   coverage/remote/index.html        mobile/remote, from npm test
#   coverage/app/index.html           mobile/app, from jest
#
#   scripts/coverage.sh               all three
#   scripts/coverage.sh rust          one of rust | remote | app
#
# Rust needs cargo-llvm-cov (`cargo install cargo-llvm-cov --locked`); the
# llvm-tools component it drives comes with rust-toolchain.toml. Its build
# goes to target/llvm-cov-target, apart from the debug build ket runs from.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/coverage"
which=${1:-all}

rust() {
  echo "› rust"
  cd "$root"
  # A failing test is reported by cargo, not a reason to have no report.
  cargo llvm-cov --workspace --ignore-run-fail --html --output-dir "$out/rust"
  cargo llvm-cov report --summary-only | tail -n 1
}

remote() {
  echo "› mobile/remote"
  cd "$root/mobile/remote"
  npm run --silent coverage
  rm -rf "$out/remote"
  mkdir -p "$out"
  cp -R coverage "$out/remote"
}

app() {
  echo "› mobile/app"
  cd "$root/mobile/app"
  npm run --silent coverage
  rm -rf "$out/app"
  mkdir -p "$out"
  if [ -d coverage ]; then cp -R coverage "$out/app"; fi
}

case "$which" in
  all) rust; remote; app ;;
  rust | remote | app) "$which" ;;
  *) echo "usage: $0 [rust|remote|app]" >&2; exit 2 ;;
esac

echo
echo "› reports in $out"
