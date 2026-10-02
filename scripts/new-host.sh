#!/usr/bin/env bash
# Replaces the running ket host with one from this checkout's build, for
# picking up host changes (phones, fitting, usage) without losing anything.
#
# A host only exits once no terminal runs in it, so by default this waits
# while any are still open in ket. `--now` (or answering y) ends them
# instead: ket reopens their tabs and resumes the agents in them, losing only
# a turn in flight.
#
#   scripts/new-host.sh [--now]
set -euo pipefail

now=""
[ "${1:-}" = "--now" ] && now=1

cd "$(dirname "$0")/.."
KET=target/debug/ket
UI=target/debug/ket-ui

echo "› building"
cargo build -p ket-ui -p ket-cli

# How many terminals the running host has; 0 when none runs.
live() {
  "$KET" host status 2>/dev/null | sed -n 's/^running: \([0-9]*\) terminal.*/\1/p' | grep . || echo 0
}

n=$(live)
if [ "$n" -gt 0 ] && [ -z "$now" ] && [ -t 0 ]; then
  # Run from a ket terminal, waiting would wait on itself.
  read -r -p "› $n terminal(s) running in ket. End them now? ket resumes their agents when it reopens [y/N] " answer
  case "$answer" in [yY]*) now=1 ;; esac
fi
if [ "$n" -gt 0 ] && [ -n "$now" ]; then
  # Through the host's socket first: it ends the terminals itself, and it
  # works from a sandboxed shell whose signals never arrive. A host older
  # than `--force` ignores it and is left running, for the signal below.
  "$KET" host stop --force >/dev/null 2>&1 || true
  for _ in $(seq 1 10); do [ "$(live)" -eq 0 ] && break; sleep 0.5; done
  n=$(live)
fi
if [ "$n" -gt 0 ] && [ -n "$now" ]; then
  # The host serving the socket, by the socket, not by name: a sandbox host
  # or an old one runs under the same name. Its terminals go with it.
  socket=$("$KET" host status | sed -n 's/^socket: *//p')
  host=""
  for pid in $(lsof -t "$socket" 2>/dev/null || true); do
    command=$(ps -o command= -p "$pid" || true)
    [[ $command == *--ket-host-serve* ]] && host=$pid && break
  done
  if [ -n "$host" ]; then
    echo "› ending the host ($host) and its $n terminal(s)"
    kill "$host" || true
    for _ in $(seq 1 10); do kill -0 "$host" 2>/dev/null || break; sleep 0.5; done
    kill -0 "$host" 2>/dev/null && kill -9 "$host" 2>/dev/null
    sleep 0.5
    if kill -0 "$host" 2>/dev/null; then
      # A sandboxed shell (Claude Code's, which `!` also uses) drops signals
      # to processes outside it, and `kill` still reports success.
      echo "› the host ($host) ignored the signal — run this from Terminal.app, not a sandboxed shell" >&2
      exit 1
    fi
  fi
elif [ "$n" -gt 0 ]; then
  echo "› $n terminal(s) still running in ket — close them there, or rerun with --now; this carries on when they are gone"
  while [ "$(live)" -gt 0 ]; do sleep 2; done
fi

# The window, not the host: a host runs as `ket-host --ket-host-serve`.
echo "› quitting ket"
for pid in $(pgrep -f "$UI" || true); do
  case "$(ps -o command= -p "$pid")" in
    *--ket-host-serve*) ;;
    *) kill "$pid" 2>/dev/null || true ;;
  esac
done

echo "› stopping the old host"
"$KET" host stop || true
for _ in $(seq 1 20); do
  "$KET" host status 2>/dev/null | grep -q '^running' || break
  sleep 0.5
done

# A relay started by hand holds the port the host now serves itself.
for pid in $(pgrep -f 'ket-relay .*:7979' || true); do
  echo "› stopping the separate relay ($pid)"
  kill "$pid" 2>/dev/null || true
done

# Phones on, as the Settings › Devices switch would leave them.
config=$("$KET" config path)
mkdir -p "$(dirname "$config")"
touch "$config"
if ! grep -q '^\[phones\]' "$config"; then
  printf '\n[phones]\nenabled = true\n' >>"$config"
  echo "› turned on Devices in $config"
fi

# Where the phone app is served, for the pairing QR code: this checkout's
# Expo dev server, whichever port it got.
web=""
expo=$(pgrep -f "$(pwd)/mobile/app/node_modules/.bin/expo start" | head -1 || true)
if [ -n "$expo" ]; then
  port=$(lsof -nP -a -p "$expo" -iTCP -sTCP:LISTEN 2>/dev/null | awk 'NR>1 {sub(/.*:/, "", $9); print $9; exit}')
  ip=$(ipconfig getifaddr en0 2>/dev/null || true)
  [ -n "$port" ] && [ -n "$ip" ] && web="http://$ip:$port"
fi
[ -z "$web" ] && echo "› the phone app's dev server is not running (cd mobile/app && npx expo start)"

echo "› opening ket"
env -u KET_RELAY_URL ${web:+KET_PHONE_WEB_URL=$web} nohup "$UI" >/dev/null 2>&1 &

for _ in $(seq 1 40); do
  "$KET" host status 2>/dev/null | grep -q '^running' && break
  sleep 0.5
done
"$KET" host status
echo "› done — reload the phone"
