#!/usr/bin/env bash
set -euo pipefail

context="${1:?script assertion context path is required}"
out="$TEMPER_SCENARIO_ARTIFACT_DIR/bounded-discovery-summary.json"

# Two terminal buckets each have the documented 64 list + 100 ambiguous-PR
# allowance. Twelve fixed open-query/coordination requests keep this scenario's
# full-pass ceiling explicit without depending on history cardinality.
provider_ceiling=340
row_ceiling=2002

jq -e \
  --argjson provider_ceiling "$provider_ceiling" \
  --argjson row_ceiling "$row_ceiling" \
  '
  .run_evidence as $run |
  ($run.observability.events // []) as $events |
  [$events[] | select(.event == "history.seeded")] as $seed |
  [$events[] | select(.event == "candidate.discovery")] as $discoveries |
  [$events[] | select(.event == "mechanical.phase")] as $phases |
  [$events[] | select(.event == "mechanical.reconciliation")] as $reconciliations |
  [$run.stimuli[]?] as $stimuli |

  ($seed | length == 1) and
  ($seed[0].fields["history.target_closed_issues"] == "220") and
  ($seed[0].fields["history.target_closed_pull_requests"] == "120") and
  ($seed[0].fields["history.sibling_closed_issues"] == "220") and
  ($seed[0].fields["history.sibling_repo"] == "acme/history-noise") and
  ($seed[0].fields["history.webhook_delivery"] | startswith("omitted:")) and
  ($seed[0].fields["history.actionable_older_than_history"] == "true") and
  ($seed[0].fields["history.actionable_recovered"] == "true") and
  ($seed[0].fields["history.cold_authority_rebuilt"] == "true") and
  (($seed[0].fields["history.actionable_pull_request_number"] | tonumber) <
    ($seed[0].fields["history.first_irrelevant_pull_request_number"] | tonumber)) and

  ($discoveries | length >= 8) and
  ([$discoveries[] | select(.fields["candidate.discovery_cache_reused"] == "false")] | length >= 2) and
  ([$discoveries[] | select(.fields["candidate.discovery_complete"] == "true")] | length >= 2) and
  ([$discoveries[] | select(
      ((.fields["candidate.continuation_bucket_count"] | tonumber) > 0) or
      ((.fields["candidate.overflow_bucket_count"] | tonumber) > 0)
    )] | length >= 1) and
  ([$discoveries[] | select(
      .fields["candidate.discovery_cache_reused"] == "true" and
      .fields["candidate.discovery_complete"] == "true" and
      .fields["candidate.consumer"] == "role"
    )] | length >= 4) and
  ([$discoveries[] | select(
      .fields["candidate.discovery_cache_reused"] == "true" and
      .fields["candidate.discovery_complete"] == "true" and
      .fields["candidate.consumer"] == "mechanical"
    )] | length >= 4) and
  ([$discoveries[] | select(
      ((.fields["candidate.retained_row_count"] | tonumber) >= 1) and
      ((.fields["candidate.hydrated_artifact_count"] | tonumber) >= 1)
    )] | length >= 1) and
  ($discoveries | all(
      .fields.outcome == "success" and
      .fields["candidate.provider_requests_available"] == "true" and
      (.fields["wake.run_id"] | length > 0) and
      ((.fields["candidate.provider_request_total"] | tonumber) <= $provider_ceiling) and
      ((.fields["candidate.raw_provider_row_count"] | tonumber) <= $row_ceiling) and
      ((.fields["candidate.hydrated_artifact_count"] | tonumber) <= 1) and
      ((.fields["candidate.exact_detail_read_count"] | tonumber) <= 1)
    )) and

  ($phases | length >= 6) and
  ($phases | all(
      .fields.outcome == "success" and
      .fields["provider.requests_available"] == "true" and
      ((.fields["provider.request_total"] | tonumber) <= $provider_ceiling)
    )) and

  ($reconciliations | length >= 6) and
  ($reconciliations | all(
      .fields["mechanical.scope"] == "broad" and
      .fields.mode == "bounded" and
      ((.fields.snapshot_count | tonumber) <= 1) and
      ((.fields.hydrated_artifact_count | tonumber) <= 1) and
      ((.fields.exact_detail_read_count | tonumber) <= 1) and
      ((.fields["detail_cache.hit_count"] | tonumber) >= 0) and
      ((.fields["detail_cache.miss_count"] | tonumber) >= 0) and
      ((.fields["detail_cache.forced_refresh_count"] | tonumber) >= 0) and
      ((.fields["detail_cache.invalidation_count"] | tonumber) >= 0) and
      ((.fields["detail_cache.eviction_count"] | tonumber) >= 0)
    )) and
  ([$reconciliations[] | select(
      ((.fields["detail_cache.hit_count"] | tonumber) > 0)
    )] | length >= 2) and
  ([$reconciliations[] | select(
      ((.fields.recovery_action_count | tonumber) > 0) and
      ((.fields.applied_action_count | tonumber) > 0)
    )] | length >= 1) and

  ($stimuli | length == 3) and
  ($stimuli | all(.status == "passed" and .attempts == 1)) and
  ([$stimuli[] | select(.action == "discovery.wait_warm")] | length == 2) and
  ([$stimuli[] | select(.action == "temper.restart")] | length == 1)
  ' "$context" >"$out"

printf 'history-independent terminal recovery verified: provider<=%s rows<=%s candidate/reconciliation hydrated<=1 exact<=1\n' \
  "$provider_ceiling" "$row_ceiling"
