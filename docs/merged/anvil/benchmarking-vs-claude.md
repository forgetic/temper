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
| t3 6-area audit (sub-agent) | claude | opus-4-8 | 568s | 65.3K† | 3.40M | **6** (Agent) | AUDIT.md 592 ln |
| t3 6-area audit (pre-fix) | anvil | opus-4-8 | 338s* | 52.3K | 638K | **6** (investigate) | AUDIT.md 523 ln* |
| t3 6-area audit (post-fix) | anvil | opus-4-8 | 471s | 50.3K | 797K | **6** (investigate) | AUDIT.md 453 ln, exit 0 |
| t4 critical review (sub-agent) | claude | opus + **haiku** | 600s | 46.6K | 4.75M | **6** (4 review + 2 Explore) | REVIEW.md 248 ln |
| t4 critical review | anvil | opus + **haiku** | **292s** | **30.4K** | 2.52M | **4** (investigate) | REVIEW.md 221 ln, exit 0 |

\* the first t3-anvil run produced a complete 523-line audit and all six
sub-agents finished, but exited non-zero on the final synthesis HTTP request
because of the skein TLS large-body bug (see below). **t3-anvil2 (post-fix) is
the validation re-run: exit 0, clean `Completed` stop, 6 sub-agents / 35
sub-agent turns, AUDIT.md the only diff.** A single mid-run `tool_error` was
recovered from without failing the run.

† **Accounting correction.** The original t3 figures counted only the *main*
trace's output tokens (26.8K) because newer Claude Code writes each `Agent`
sub-agent's turns to a separate `subagents/agent-*.jsonl` file, not inline as
`isSidechain` rows. Folding those in (`analyze_claude.py` now does this
automatically) gives the true total: **65.3K output / 62 sub-agent turns / 3.40M
cache_read**. The earlier "anvil uses ~2× output" conclusion was an artifact of
comparing anvil's full total against Claude's main-only total. On a like-for-like
basis **anvil used fewer output tokens than Claude on the same audit** (50.3K vs
65.3K) with the identical 6-way fan-out.

## Key finding: sub-agent fan-out and model tiering match Claude

On both the audit (t3) and the critical-review (t4) tasks, both agents fan out
independent sub-agents in a single turn to investigate the independent areas in
parallel, then synthesize the report themselves:

- Claude: one turn issues several `Agent`/`Explore` tool calls at once. On t4 it
  spawned **4 `general-purpose` review sub-agents on Opus + 2 `Explore`
  investigation sub-agents on Haiku** — and the review sub-agents recursively
  spawned their own `Explore` sub-agents (nested delegation).
- Anvil: turn 0 orients (`ls`/`find` in parallel), the next turn issues several
  `investigate` calls in one response — the read-only sub-agent tool is
  parallel-safe so the machine runs them concurrently.

**Model tiering.** The decisive efficiency lever the t4 trace revealed: Claude
runs its investigation sub-agents on **Haiku** while the orchestrator stays on
**Opus**. Anvil now does the same — the `investigate` sub-agent runs on a
configurable cheaper tier (`claude-haiku-4-5` by default under Anthropic OAuth,
`TEMPER_AGENTS_ANTHROPIC_SUBAGENT_MODEL` to override; non-Anthropic providers
keep one model). On t4 this is what brings anvil to **292s / 30.4K output**
versus Claude's **600s / 46.6K** for an equivalent 221-line review — faster and
leaner, same architecture, clean exit.

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

## Changes made — t4 critical-review round (sub-agent model tiering + transport)

The t4 review task (drive a parallel critical code review, the richest
sub-agent pattern seen in real Claude traces — multi-area, multi-round,
recursively-delegating reviewers) surfaced four more gaps, each now fixed:

Sub-agent model tiering (the efficiency win)
- **anvil**: the `investigate` sub-agent now runs on a cheaper model tier than
  the orchestrator (`ProviderConfig::subagent_model_id()` /
  `with_model_id()`), defaulting to `claude-haiku-4-5` under Anthropic OAuth,
  overridable via `TEMPER_AGENTS_ANTHROPIC_SUBAGENT_MODEL`. This mirrors Claude
  Code routing `Explore` sub-agents to Haiku. Non-Anthropic providers keep one
  model (no cheap tier wired up).
- **anvil**: Anthropic `max_tokens` and the `anthropic-beta` flags are now
  **model-aware**. Haiku caps output at 64K (Opus/Sonnet 128K) and is NOT
  entitled to the `context-1m` long-context beta on the standard tier —
  requesting either 400s every request. `max_output_tokens_for` /
  `context_window_for` / `anthropic_beta_for` pick per model, and the sub-agent
  factory rebuilds the headers for its own (smaller) model instead of
  inheriting the parent's. The sub-agent iteration budget rose 12 → 24 because
  the smaller model takes more, smaller steps per investigation.

Transport robustness (the real "connection reset / hang" root cause)
- **skein tls**: the prior `WriteZero` fix was incomplete. `poll_write` of a
  body larger than rustls' plaintext send cap (~128KB) returned `Poll::Pending`
  **without registering a waker** once rustls stopped accepting plaintext but
  the socket had drained cleanly — deadlocking the task forever (the real cause
  of the "connection reset by peer" / indefinite hangs on LLM sub-agent and
  synthesis turns; curl over the same HTTP/1.1 path had no trouble at any
  size). It now `continue`s to feed more plaintext after a clean drain. Verified
  with `tongs`' `anthropic_reset_repro` example: hangs at ≥140KB before, passes
  through 600KB after, 10×250KB stress clean.
- **anvil-agent**: model calls now have a **liveness timeout** (no first
  response within 120s, or no stream event for 120s → fail the turn instead of
  hanging) and **transient-error retry** with capped exponential backoff (up to
  6 retries on transport faults / 429 / 5xx; never on 400/401/404/auth/decode —
  see `is_retryable`). A stalled or flaky sub-agent call no longer deadlocks the
  whole parallel batch. Failures are logged as `model_call_failed` (scope,
  reason, will_retry) for observability.

## Model availability note

Mid-benchmark, Anthropic suspended Fable 5 access on this subscription. Requests
for `claude-fable-5` started returning `404 "Claude Fable 5 is not available.
Please use Opus 4.8."`. Claude Code masks this with a transparent fallback;
anvil sends the literal model id, so it 404'd. Handled two ways: default reverted
to opus-4-8, and the failure is now reported transparently (see Robustness).
