#!/usr/bin/env bash
# Project:  Privatium™  |  File: .github/scripts/fresh-clone.sh
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-05
# Summary:  The fresh-clone check of docs/plans/phase-1.md M13: `cargo build`, then the
#           binary, given nothing but an empty data directory, serves the hello app at
#           http://127.0.0.1:8420 — the Phase 1 deliverable of docs/roadmap.md, from a
#           checkout, with no configuration. Bash on all three CI platforms.

set -euo pipefail

data_dir="${1:-${RUNNER_TEMP:-/tmp}/fresh-node}"
rm -rf "$data_dir"

cargo build --locked
bin="target/debug/privatium"
[[ -f "$bin.exe" ]] && bin="$bin.exe"

log="$(mktemp)"
"$bin" --data-dir "$data_dir" >"$log" 2>&1 &
pid=$!
# `kill` reaches a native child from Git Bash too; `$!` is bash's own pid for it, which
# is not the Windows pid `taskkill` would want, so that is not the tool here.
stop() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
trap stop EXIT

for _ in $(seq 1 120); do
  grep -q 'listening on http://127.0.0.1:8420/' "$log" && break
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "the node exited before listening:"
    cat "$log"
    exit 1
  fi
  sleep 0.5
done
if ! grep -q 'listening on http://127.0.0.1:8420/' "$log"; then
  echo "the node never announced http://127.0.0.1:8420/:"
  cat "$log"
  exit 1
fi

launcher="$(curl -sS --fail http://127.0.0.1:8420/)"
if ! grep -q 'Hello' <<<"$launcher"; then
  echo "the launcher does not list hello:"
  echo "$launcher"
  exit 1
fi
hello="$(curl -sS --fail http://127.0.0.1:8420/a/hello/)"
if ! grep -q "We haven't met yet." <<<"$hello"; then
  echo "hello did not render its greeting:"
  echo "$hello"
  exit 1
fi
echo "fresh clone: cargo build, then hello at http://127.0.0.1:8420/a/hello/"
