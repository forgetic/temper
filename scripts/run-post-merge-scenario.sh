#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <pull-request> <merged-main-sha> <artifact-directory>" >&2
  exit 2
}

if [ "$#" -ne 3 ]; then
  usage
fi

pull_request="$1"
merged_sha="$2"
artifact_dir="$3"
state_ref="refs/temper/scenario-validation-state"
state_file="$(mktemp "${TMPDIR:-/tmp}/temper-scenario-state.XXXXXX")"
trap 'rm -f -- "$state_file"' EXIT

old_state_commit="$(git ls-remote --refs origin "$state_ref" | awk 'NR == 1 { print $1 }')"
if [ -n "$old_state_commit" ]; then
  git fetch --no-tags origin "+${state_ref}:${state_ref}"
  git show "${state_ref}:state.json" > "$state_file"
else
  printf '%s\n' '{"schema":"temper.scenario-validation-state.v1","scenarios":{}}' > "$state_file"
fi

mapfile -t active_scenarios < <(
  for manifest in scenarios/*/scenario.toml; do
    status="$(sed -n 's/^status = "\([^"]*\)"/\1/p' "$manifest" | head -1)"
    if [ "$status" = "active" ]; then
      dirname "$manifest"
    fi
  done | LC_ALL=C sort
)

if [ "${#active_scenarios[@]}" -eq 0 ]; then
  echo "no active checked-in scenarios found" >&2
  exit 1
fi

selected_scenario="$({
  for scenario_path in "${active_scenarios[@]}"; do
    scenario_name="${scenario_path#scenarios/}"
    attempted_at="$(jq -r --arg name "$scenario_name" \
      '.scenarios[$name].last_attempted_at // "0000-00-00T00:00:00Z"' "$state_file")"
    printf '%s\t%s\t%s\n' "$attempted_at" "$scenario_name" "$scenario_path"
  done
} | LC_ALL=C sort -k1,1 -k2,2 | head -1 | cut -f3)"

selected_name="${selected_scenario#scenarios/}"
attempted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
mkdir -p "$artifact_dir"
printf '%s\n' "$selected_scenario" > "$artifact_dir/selected-scenario.txt"
printf '%s\n' "$old_state_commit" > "$artifact_dir/previous-state-commit.txt"
echo "Selected least-recently-run scenario: $selected_scenario"

set +e
cargo run -p temper-scenario-cli -- validate \
  --pr "$pull_request" \
  --sha "$merged_sha" \
  --scenario "$selected_scenario" \
  --output-dir "$artifact_dir"
validation_status=$?
set -e

completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
result="failure"
if [ "$validation_status" -eq 0 ]; then
  result="success"
fi

next_state_file="${state_file}.next"
jq \
  --arg name "$selected_name" \
  --arg path "$selected_scenario" \
  --arg sha "$merged_sha" \
  --arg pr "$pull_request" \
  --arg attempted_at "$attempted_at" \
  --arg completed_at "$completed_at" \
  --arg result "$result" \
  '.scenarios[$name] = ((.scenarios[$name] // {}) + {
      path: $path,
      last_attempted_sha: $sha,
      last_attempted_pr: ($pr | tonumber),
      last_attempted_at: $attempted_at,
      last_completed_at: $completed_at,
      last_result: $result
    })
    | if $result == "success" then
        .scenarios[$name] += {
          last_successful_sha: $sha,
          last_successful_pr: ($pr | tonumber),
          last_successful_at: $completed_at
        }
      else . end' "$state_file" > "$next_state_file"
mv "$next_state_file" "$state_file"
cp "$state_file" "$artifact_dir/scenario-validation-state.json"

state_blob="$(git hash-object -w "$state_file")"
state_tree="$(printf '100644 blob %s\tstate.json\n' "$state_blob" | git mktree)"
parent_args=()
lease="--force-with-lease=${state_ref}:"
if [ -n "$old_state_commit" ]; then
  parent_args=(-p "$old_state_commit")
  lease="--force-with-lease=${state_ref}:${old_state_commit}"
fi
state_commit="$(
  printf 'post-merge: record %s for PR #%s\n' "$selected_name" "$pull_request" |
    GIT_AUTHOR_NAME="Temper scenario scheduler" \
    GIT_AUTHOR_EMAIL="temper-scenario-scheduler@localhost" \
    GIT_COMMITTER_NAME="Temper scenario scheduler" \
    GIT_COMMITTER_EMAIL="temper-scenario-scheduler@localhost" \
    git commit-tree "$state_tree" "${parent_args[@]}"
)"
git push origin "${state_commit}:${state_ref}" "$lease"
printf '%s\n' "$state_commit" > "$artifact_dir/state-commit.txt"

if [ "$validation_status" -ne 0 ]; then
  echo "scenario validation failed with status $validation_status" >&2
fi
exit "$validation_status"
