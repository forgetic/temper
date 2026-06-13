# Anvil vs Claude Code — benchmarking results

Goal: prove anvil can execute coding workloads like the Claude Code agent —
similar wall-clock and token usage, and the same tool-usage / sub-agent
spawning behavior — and fix the anvil/tongs/skein bugs surfaced along the way.

Method: drive `anvil-agent` exactly as `smith-worker` would (the smith agent
process protocol — `TEMPER_CODING_WORKSPACE_CONTEXT`/`RESULT` env, work-item
JSON, run in a prepared git checkout with a local bare origin for checkpoint
pushes), and run `claude -p --output-format json --dangerously-skip-permissions`
on the same task in a parallel scratch repo. Both on the Anthropic
subscription, model `claude-opus-4-8` (Fable 5 was suspended mid-run — see
"model availability" below). Harness: `run_anvil.sh`, `run_claude.sh`,
`analyze_claude.py`.

## Results

| Task | Agent | Model | Wall | out tok | cache_read | sub-agents | Tests / product |
|------|-------|-------|------|---------|------------|------------|-----------------|
| t1 RFC3339 parser (simple) | claude | fable-5 | 96s | 7.8K | 92K | 0 | 27 tests OK |
| t1 RFC3339 parser | anvil  | fable-5 | 119s | 6.7K | 98K | 0 | 27 tests OK |
| t2 sans-IO HTTP parser (medium) | claude | fable-5 | 434s | 34.5K | 628K | 0 | 73 tests OK |
| t2 sans-IO HTTP parser | anvil  | fable-5 | 266s | 21.1K | 192K | 0 | 62 tests OK |
| t3 6-area audit (sub-agent) | claude | opus-4-8 | 568s | 26.8K | 590K | **6** (Agent) | AUDIT.md 592 ln |
| t3 6-area audit (pre-fix) | anvil | opus-4-8 | 338s* | 52.3K | 638K | **6** (investigate) | AUDIT.md 523 ln* |
| t3 6-area audit (post-fix) | anvil | opus-4-8 | 471s | 50.3K | 797K | **6** (investigate) | AUDIT.md 453 ln, exit 0 |

\* the first t3-anvil run produced a complete 523-line audit and all six
sub-agents finished, but exited non-zero on the final synthesis HTTP request
because of the skein TLS `WriteZero` bug. **t3-anvil2 (post-fix) is the
validation re-run: exit 0, clean `Completed` stop, 6 sub-agents / 35 sub-agent
turns, AUDIT.md the only diff, 797K cache_read confirming caching on a large
multi-turn run.** A single mid-run `tool_error` was recovered from without
failing the run.

## Key finding: sub-agent fan-out matches Claude

On the audit task both agents spawned **6 sub-agents in a single turn** to
investigate the 6 independent areas in parallel, then synthesized the report
themselves:

- Claude: turn issues 6 `Agent` (general-purpose) tool calls at once.
- Anvil: turn 0 orients (`ls` + `find` in parallel), turn 1 issues **6
  `investigate` tool calls in one response** — the read-only sub-agent tool is
  parallel-safe so the machine runs them concurrently. 34 sub-agent turns total
  across the 6 investigations.

Anvil reached parity here only after the changes below; before them it had no
caching, no usage visibility, no delegation guidance, and a broken build.

## Changes made (anvil / tongs / skein)

Build / runtime
- **anvil-io-engine**: skein dropped `Runtime::current_handle()`; anvil now owns
  an ambient engine-handle slot (installed per runtime thread via
  `on_thread_start`, cleared on drop) so sub-agent spawning resolves a handle.
  Without this anvil did not compile. (`fd2c93e`)

Efficiency (the token/wall-clock parity work)
- **tongs anthropic**: added prompt-caching `cache_control` breakpoints — one on
  the system/tools prefix, two sliding on the last user-role turns. Before this
  `cache_read` was 0 for whole runs (every turn re-billed the full conversation
  as fresh input). (`9fabc78`)
- **anvil coding agent**: added EFFICIENCY guidance (batch independent read-only
  tool calls, write files whole, verify once) and SUB-AGENTS guidance (when to
  delegate, launch several investigations in one response, self-contained
  tasks) to the role prompt — mirroring Claude Code's behavioral guidance.
  (`3250561`)

Observability
- **tongs/anvil**: per-turn token usage now flows as `AgentEvent::TurnUsage`;
  `UsageLogger` logs `turn_usage` / `tool_start` / `agent_end` structured stderr
  lines and a per-run `usage_total`, across the main run and all sub-agents.
  This is what makes runs comparable. (`4f2c330`)

Robustness
- **skein tls**: `poll_write` returned `Ok(0)` (→ `WriteZero`) on bodies larger
  than rustls' bounded plaintext buffer (~600KB synthesis requests). Now drains
  encrypted bytes and retries; never returns `Ok(0)` for a non-empty buf.
  (skein `f2ef7b9`)
- **anvil**: model-unavailability (e.g. Fable 5 suspended → 404 "use Opus 4.8")
  is now a dedicated `ModelUnavailable` error that names the model and the
  override env vars, tagged `model-unavailable:` on stderr — instead of an
  opaque abnormal stop. (`a63ecb8`)
- **anvil**: default Anthropic OAuth model is `claude-opus-4-8` (the tier serves
  it; Fable 5 is gated/suspended). Override with `TEMPER_AGENTS_ANTHROPIC_MODEL`.
  (`ff1ccd5`)

## Model availability note

Mid-benchmark, Anthropic suspended Fable 5 access on this subscription. Requests
for `claude-fable-5` started returning `404 "Claude Fable 5 is not available.
Please use Opus 4.8."`. Claude Code masks this with a transparent fallback;
anvil sends the literal model id, so it 404'd. Handled two ways: default reverted
to opus-4-8, and the failure is now reported transparently (see Robustness).
