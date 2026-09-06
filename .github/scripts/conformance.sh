#!/usr/bin/env bash
# Project:  Privatium™  |  File: .github/scripts/conformance.sh
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-06
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
# Cluster secret exclusion and the certificate lifetime; renewal on sync remains planned.
run privatium-core identity \
  test_spec_2_3_3_cluster_private_key_is_absent_from_every_event_snapshot_and_backup \
  test_spec_2_3_1_certificate_verifies_against_the_cluster_key_and_expires_at_180_days \
  test_spec_2_3_1_certificate_renews_under_ninety_days \
  test_spec_3_1b_data_only_restore_preserves_records_and_selects_local_identity \
  test_spec_3_1b_restored_keys_select_the_original_cluster \
  test_spec_3_1b_replayed_rows_cannot_change_local_cluster_identity

# Session primitives and handshake refusals; the live channel has its own acceptance.
run privatium-core session \
  test_spec_8_key_schedule_matches_the_checked_in_vectors \
  test_spec_8_frames_round_trip_and_the_counter_never_repeats \
  test_spec_8_a_tampered_frame_is_refused \
  test_spec_8_handshake_derives_the_same_keys_on_both_sides \
  test_spec_8_1_a_static_key_that_is_not_the_pinned_one_fails_the_confirm \
  test_spec_8_3_unknown_revoked_and_missing_device_keys_are_refused

# Pairing (§7): owner action, the node-generated 16-bit code with its two renderings and
# the glyphs' variation selectors, the 120 s TTL and five attempts, no bearer code on the
# wire, and the device row a success writes — replica declared, public key only.
run privatium-core pair \
  test_spec_7_1_pairing_is_closed_until_opened_and_closes_on_first_success \
  test_spec_7_2_code_is_16_bits_rendered_as_four_glyphs_and_two_words \
  test_spec_7_2_word_input_is_case_and_punctuation_insensitive \
  test_spec_7_2_glyph_labels_are_accepted_as_input \
  test_spec_7_3_glyph_table_is_normative_and_keeps_variation_selectors \
  test_spec_7_5_code_expires_at_120s_and_five_attempts_issue_a_new_one \
  test_spec_7_0_the_code_never_crosses_the_wire \
  test_spec_7_4_pairing_completes_and_writes_the_device_row

# The bootstrap exposes no application content, and live sessions stream through handle.
run privatium-core channel \
  test_spec_8_4_plain_http_on_the_lan_serves_only_the_bootstrap_set \
  test_spec_9_2_bootstrap_page_carries_no_app_data \
  test_spec_8_3_page_frame_scripts_carry_integrity \
  test_spec_8_3_1_bootstrap_uses_destination_app_permissions
run privatium channel \
  test_spec_8_2_lan_socket_carries_no_plaintext_app_data \
  test_channel_streams_a_response_body_frame_by_frame \
  test_spec_8_3_browser_client_against_live_core \
  test_spec_8_3_1_handoff_survives_disconnect_without_repeating_a_write \
  test_spec_8_3_1_wrong_device_cannot_consume_or_release_a_response \
  test_spec_8_3_1_capacity_refuses_before_dispatch_and_release_frees_it

# Discovery (§6): the full TXT key set advertised and browsable (§6.1), every configured
# mechanism started concurrently rather than chained (§6.5), and the UDP responder
# refusing a non-private source (§6.4).
run privatium-core discover \
  test_spec_6_1_txt_record_carries_the_full_key_set_and_stays_under_1300_bytes \
  test_spec_6_1_mdns_registration_is_browsable_and_keyed_by_id \
  test_spec_6_5_mdns_and_udp_start_together_and_stop_together \
  test_spec_6_4_udp_refuses_a_public_source_and_answers_once_a_second

echo "conformance: Phase 1, identity, pairing, encrypted-channel and discovery items hold by name"
