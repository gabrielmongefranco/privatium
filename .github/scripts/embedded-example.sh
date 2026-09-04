#!/usr/bin/env bash
# Project:  Privatium™  |  File: .github/scripts/embedded-example.sh
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-05
# Summary:  Run examples/embedded.rs the way an embedder would (spec/app-contract.md §2.3,
#           docs/plans/phase-1.md M13): a fresh data directory, one append and one query
#           printed, an own axum router behind auth_layer answering on loopback and
#           refusing a foreign Host. Bash on all three CI platforms; `curl` is preinstalled.

set -euo pipefail

data_dir="${1:-${RUNNER_TEMP:-/tmp}/embedded-node}"
rm -rf "$data_dir"

# Build first and run the binary itself, so the process in the background is the example
# and not a `cargo run` whose child would outlive it on Windows.
cargo build -p privatium-core --locked --example embedded
bin="target/debug/examples/embedded"
[[ -f "$bin.exe" ]] && bin="$bin.exe"

log="$(mktemp)"
"$bin" "$data_dir" >"$log" 2>&1 &
pid=$!
# `kill` reaches a native child from Git Bash too; `$!` is bash's own pid for it, which
# is not the Windows pid `taskkill` would want, so that is not the tool here.
stop() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
trap stop EXIT

url=""
for _ in $(seq 1 120); do
  url="$(grep -o 'http://127.0.0.1:[0-9]*/' "$log" | head -n 1 || true)"
  [[ -n "$url" ]] && break
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "the example exited before listening:"
    cat "$log"
    exit 1
  fi
  sleep 0.5
done
if [[ -z "$url" ]]; then
  echo "the example never announced a URL:"
  cat "$log"
  exit 1
fi

# The append and the query, as the spec shows them: one row, points typed as a string.
if ! grep -q '^1 score(s): \[{"player":"ada","points":"42"}\]$' "$log"; then
  echo "the append or the query is not what spec/app-contract.md §2.3 shows:"
  cat "$log"
  exit 1
fi

body="$(curl -sS --fail "$url")"
if [[ "$body" != "an embedder's own route" ]]; then
  echo "unexpected body from the embedder's route: $body"
  exit 1
fi
status="$(curl -sS -o /dev/null -w '%{http_code}' -H 'Host: evil.example' "$url")"
if [[ "$status" != "403" ]]; then
  echo "a foreign Host was answered $status, not 403 (auth_layer, spec/app-contract.md §6)"
  exit 1
fi
echo "embedded example: appended, queried, served $url, refused a foreign Host"
