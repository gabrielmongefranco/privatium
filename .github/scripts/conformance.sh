#!/usr/bin/env bash
# Project:  Privatium™  |  File: .github/scripts/conformance.sh
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-05
# Summary:  docs/plans/phase-1.md §7: the lines of spec/protocol.md §13 a Phase 1 build can
#           satisfy, asserted by name — plus the two docs/roadmap.md bullets §7 says are
#           easy to lose (every route through core::handle; bodies stream both ways). The
#           test binaries were built by the test step; each name below must run and pass,
#           and a name that matches nothing fails here rather than passing by absence.

set -euo pipefail

# run <package> <test binary> <test name>...: every name, exactly, and as many passes.
run() {
  local package="$1" binary="$2"
  shift 2
  local expected=$#
  local out
  out="$(cargo test -p "$package" --locked --test "$binary" -- --exact "$@" 2>&1)" || {
    echo "$out"
    exit 1
  }
  local passed
  passed="$(grep -o '[0-9]* passed' <<<"$out" | tail -n 1 | cut -d' ' -f1)"
  if [[ "$passed" != "$expected" ]]; then
    echo "$out"
    echo "conformance: $binary: expected $expected named tests to run, $passed passed"
    exit 1
  fi
  printf 'conformance: %s: %s\n' "$binary" "$*"
}

# Deleting cache/ and every snap/ loses no data (§3.1, §5); LWW by (lam, ts, dev) (§4.5);
# an event past the horizon never wins a row (§4.4, the materialization half).
run privatium-core store \
  test_spec_3_1_delete_cache_loses_nothing \
  test_spec_4_5_lww_by_lam_ts_dev \
  test_spec_4_4_future_event_does_not_win_the_row
# Unknown fields byte for byte (§4.2); Lamport monotonic across restart (§4.3, the restart
# half); events more than 24 h in the future rejected (§4.4, the log-scan half).
run privatium-core log \
  test_spec_4_2_unknown_fields_preserved \
  test_spec_4_3_lamport_survives_restart \
  test_spec_4_4_future_ts_rejected
# Three-tier read fallback with the tier recorded (§5.3); the oldest snapshot never
# pruned (§5.4).
run privatium-core snapshot \
  test_spec_5_3_tier1_sqlite \
  test_spec_5_3_tier2_on_sqlite_corruption \
  test_spec_5_3_tier3_on_csv_corruption \
  test_restore_reports_tier_used \
  test_spec_5_4_never_prunes_oldest
# Unauthenticated endpoints leak no app data (§9.2); every route through core::handle with
# no socket, and a response body that streams (docs/roadmap.md, ADR 0003).
run privatium-core wire \
  test_spec_9_2_unauthenticated_leaks_nothing \
  test_spec_9_1_every_prefix_reachable_through_handle \
  test_response_body_streams_without_buffering
# Apps declaring a higher api refused (§12).
run privatium-core apps test_spec_12_higher_api_refused
# The adapter never buffers a request body whole (docs/roadmap.md).
run privatium adapter \
  test_large_request_body_never_fully_buffered \
  test_response_body_streams_without_buffering
echo "conformance: every Phase 1 item of spec/protocol.md §13 holds by name"
