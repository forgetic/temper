# Process-output capture inventory

This inventory records every production subprocess reader and every test/support
use of `std::process::Command::output` as of issue #455. “Complete” means an
overflow is an error; “tail” means the retained byte count is bounded and the
dropped-byte count is reported. No Temper test invokes `cargo test` recursively.

## Production readers

| Owner / source | Data contract | Status | Limit / rationale |
| --- | --- | --- | --- |
| `temper-agent-core/src/managed_bash.rs` | Human tool output | Bounded tail | 256 KiB rolling bytes, then 50 KiB / 2,000 rendered lines |
| `temper-agent/src/mcp/connection.rs` stdout | Newline-delimited JSON-RPC | Migrated: bounded complete records + bounded queue | 1 MiB per inbound record; 16 queued records; oversized lines are drained and cause typed `McpError::ProtocolOverflow` |
| `temper-agent/src/mcp/connection.rs` stdin | Newline-delimited JSON-RPC | Migrated: bounded complete record | 1 MiB including delimiter; serialization fails with typed protocol overflow before writing |
| `temper-agent/src/coding_agent/result.rs` git status | Porcelain filenames | Migrated: bounded complete | 1 MiB; overflow is rejected, never interpreted as a partial status |
| `temper-agent/src/coding_agent/result.rs` git tree comparison | Filename diff | Migrated: bounded complete | 1 MiB; overflow is rejected, never interpreted as a partial diff |
| `temper-worker/src/managed_effect.rs` stdout | Git filenames, hashes, porcelain, patches, and protocol output | Migrated: bounded complete | 16 MiB; readers continue draining and command fails explicitly on overflow |
| `temper-worker/src/managed_effect.rs` stderr | Git/helper diagnostics | Migrated: bounded tail | 64 KiB; errors report dropped bytes |
| `temper-worker/src/pre_push/fingerprint.rs` | Hashes, NUL filenames, binary diffs | Migrated via `ManagedCommand` complete mode | 16 MiB; fingerprint creation fails on overflow |
| `temper-worker/src/pre_push/process.rs` stdout/stderr | Configured gate diagnostics | Bounded tail | 8 KiB per stream; result reports dropped bytes |
| `temper-worker/src/out_of_process_runner/stderr.rs` | Agent diagnostics | Bounded line + tail | 16 KiB per rendered line and 2 KiB retained tail; stream is always drained |
| `temper-worker/src/pr_freshness.rs` | HTTP + JSON protocol response | Migrated: bounded complete | 1 MiB; malformed or overflowing responses fail freshness closed |
| `temper-process-containment/src/linux/protocol.rs` | Supervisor control protocol | Bounded / fixed fields | Protocol parser applies bounded structured identity and diagnostic contracts |

Workspace commit, push, recovery, status, and preparation paths all converge on
`workspace/git.rs`, which explicitly selects complete stdout and tail stderr.
Fingerprinting selects the same policy independently at its call site.

## Test and test-support `Command::output` uses

`Command::output` itself has no byte limit. The table distinguishes the exact
migration required by #455 from uses whose test fixture or command has a fixed
response, and from larger harness captures that remain visibly audited rather
than being mistaken for production readers.

| Source | Bound status | Reason / disposition |
| --- | --- | --- |
| `temper-agent-session/tests/non_completed_stop.rs` abort regression | **Migrated** | Contained spawn, concurrent 64 KiB tails, 5 s process deadline, and joined cleanup |
| `temper-agent-session/tests/non_completed_stop.rs` one-iteration budget case | Inherently fixed fixture | One Jig response/tool round and `max-iterations=1` |
| `temper-agent/tests/support/coding_agent_workspace.rs` | Inherently fixed fixture | Local git commands over a bounded temporary workspace |
| `temper-scenario-cli/tests/{cli,run_evidence,run_report,validate_workflow}.rs` | Inherently fixed fixture | CLI report/help JSON generated from checked-in bounded scenarios |
| `temper-testing/src/live_basic_delivery/process.rs` | Not byte-bounded (audited) | Live capstone child; output is retained for failure evidence; not a production service reader |
| `temper-testing/src/real_stack/git.rs` | Inherently fixed command | Hash/ref queries over a bounded hermetic fixture |
| `temper-testing/tests/{daemon_worker,hermetic_real_stack/budget_retry}.rs` | Inherently fixed fixture | Hermetic daemon/worker fixtures and bounded budget cases |
| `temper-worker/tests/coding_executor/pr_repair.rs` | Inherently fixed fixture | Git assertions over a small temporary checkout |
| `temper-worker/tests/coding_executor/support/{assertions,fake_agent,target_branch}.rs` | Inherently fixed fixture | Fixed fake-agent output and local hash/status assertions |
| `temper-worker/tests/coding_worker_e2e/support.rs` | Inherently fixed fixture | Fixed fake agent and local temporary git repositories |
| `temper-worker/tests/coding_worker_lifecycle/support.rs` | Inherently fixed fixture | Local hash/status assertions over bounded lifecycle fixtures |
| `temper-worker/tests/{pre_push,workspace}.rs` | Inherently fixed fixture | Script markers and local git output generated by each test |
| `tests/check_cli/{agent_traces,support}.rs` | Inherently fixed fixture | CLI output from checked-in trace fixtures |
| `tests/{config_paths_cli,config_schema_cli,config_show_cli,operator_help_contract,plan_cli,systemd_examples}.rs` | Inherently fixed command | Compiled help/schema/config or checked-in example output |
| `tests/target_ux/support.rs` | Not byte-bounded (audited) | General CLI assertion helper; callers use bounded checked-in fixtures |

The two audited general harness captures are intentionally listed as such. They
do not read attempt-owned production children, and changing their diagnostics is
outside #455; future harness work should use `BoundedCapture` rather than adding
another `Command::output` call.
