#!/usr/bin/env bash
set -euo pipefail

context="${1:?script assertion context path is required}"
repo_root="$(git rev-parse --show-toplevel)"
head="$(git -C "$repo_root" rev-parse HEAD)"
probe_dir="$TEMPER_SCENARIO_ARTIFACT_DIR/cli-probes"
mkdir -p "$probe_dir"

# The process that launched this hook is the feature-built temper-scenario
# binary. Fall back to Cargo's default target path for environments without
# Linux procfs.
scenario_cli="$(readlink -f "/proc/$PPID/exe" 2>/dev/null || true)"
if [ "$(basename "$scenario_cli")" != "temper-scenario" ] || [ ! -x "$scenario_cli" ]; then
  scenario_cli="$repo_root/target/debug/temper-scenario"
fi
test -x "$scenario_cli"
printf '%s\n' "$scenario_cli" >"$probe_dir/temper-scenario-binary.txt"

# The successful outer run is the no-tier execution proof. Require its fixed
# live contract and retained mapping/binary/topology/Jig/observability/artifact
# facts rather than launching a nested live stack.
jq -e \
  --arg head "$head" \
  --arg topology 'real Forgejo + host `forgejo-runner` CI + standalone Temper + Jig fake-LLM agents' \
  '
    .runner_id == "manifest" and
    .tier == "live" and
    .run_evidence.verdict == "passed" and
    .run_evidence.scenario.name == "implicit-live-scenario-cli" and
    .run_evidence.scenario.feature == "ai/temper#824" and
    .run_evidence.scenario.plan == "ai/temper#825" and
    .run_evidence.scenario.source_branch == "agent/pr-for-feature-824" and
    .run_evidence.scenario.checkout_head_sha == $head and
    (.run_evidence.scenario.resolved_content_digest | test("^sha256:[0-9a-f]{64}$")) and
    .run_evidence.scenario.runner_id == "manifest" and
    .run_evidence.scenario.tier == "live" and
    .run_evidence.scenario.tier_description == $topology and
    .run_evidence.scenario.topology.kind == "single-repo-forgejo-standalone" and
    .run_evidence.scenario.topology.forge == "forgejo" and
    .run_evidence.scenario.topology.runner == "forgejo-actions-host" and
    .run_evidence.scenario.topology.temper == "standalone" and
    .run_evidence.scenario.topology.agent_model == "scripted-fake-llm" and
    (.run_evidence.binary.path | length > 0) and
    (.run_evidence.binary.sha256 | test("^[0-9a-f]{64}$")) and
    (.run_evidence.binary.size_bytes > 0) and
    (.run_evidence.provider.forgejo_url | startswith("http")) and
    (.run_evidence.provider.temper_binary | length > 0) and
    (.run_evidence.provider.fake_llm_url | startswith("http")) and
    (.run_evidence.provider.jig_script_paths | any(endswith("jig/implicit-live-scenario-cli.json"))) and
    (.run_evidence.provider.request_count > 0) and
    .run_evidence.observability.log_format == "json" and
    (.run_evidence.observability.captured_events > 0) and
    .run_evidence.assertions.status == "passed" and
    (.run_evidence.artifacts.log_paths | length > 0) and
    (.run_evidence.artifacts.artifact_paths | length > 0)
  ' "$context" >"$probe_dir/outer-run-contract.json"

for command in run validate validate-pr; do
  if ! "$scenario_cli" "$command" --help \
    >"$probe_dir/$command-help.stdout" \
    2>"$probe_dir/$command-help.stderr"; then
    echo "$command --help failed" >&2
    exit 1
  fi
  if grep -Fq -- '--tier' \
    "$probe_dir/$command-help.stdout" "$probe_dir/$command-help.stderr"; then
    echo "$command --help still exposes --tier" >&2
    exit 1
  fi
done

assert_legacy_usage_error() {
  command="$1"
  label="$2"
  shift 2
  stdout="$probe_dir/$command-$label.stdout"
  stderr="$probe_dir/$command-$label.stderr"
  status_file="$probe_dir/$command-$label.status"

  set +e
  "$scenario_cli" "$command" "$@" >"$stdout" 2>"$stderr"
  status=$?
  set -e
  printf '%s\n' "$status" >"$status_file"

  if [ "$status" -ne 64 ]; then
    echo "$command $label returned $status instead of usage status 64" >&2
    exit 1
  fi
  if [ -s "$stdout" ]; then
    echo "$command $label wrote stdout before rejecting the legacy option" >&2
    exit 1
  fi
  grep -Fq 'unexpected' "$stderr"
  grep -Fq -- '--tier' "$stderr"
}

for command in run validate validate-pr; do
  assert_legacy_usage_error "$command" tier-live --tier live
  assert_legacy_usage_error "$command" tier-hermetic --tier hermetic
  assert_legacy_usage_error "$command" tier-equals-live --tier=live
done

cargo_config="$repo_root/.cargo/config.toml"
dev_driver="$repo_root/crates/temper-dev/src/bin/temper-dev.rs"
mapfile -t aliases < <(grep -E '^dev-scenario-run[[:space:]]*=' "$cargo_config")
if [ "${#aliases[@]}" -ne 1 ]; then
  echo "expected exactly one dev-scenario-run Cargo alias" >&2
  exit 1
fi
case "${aliases[0]}" in
  *'-- dev-scenario-run"') ;;
  *)
    echo "Cargo scenario alias does not dispatch the unsuffixed internal command" >&2
    exit 1
    ;;
esac

internal_dispatches="$(grep -Fc 'OsStr::new("dev-scenario-run")' "$dev_driver")"
if [ "$internal_dispatches" -ne 1 ]; then
  echo "expected exactly one unsuffixed temper-dev scenario command dispatch" >&2
  exit 1
fi
removed_alias="dev-scenario-run-live"
if grep -Fq "$removed_alias" "$cargo_config" "$dev_driver"; then
  echo "live-suffixed scenario-run alias is still exposed" >&2
  exit 1
fi

printf '%s\n' "${aliases[0]}" >"$probe_dir/scenario-run-alias.txt"
echo "implicit-live CLI, evidence topology, and single scenario-run alias verified"
