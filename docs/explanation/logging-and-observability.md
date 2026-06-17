# Logging and observability design

Status: proposed · Audience: operators, agents consuming logs, contributors adding emit sites

This document defines temper's logging UX and the structured-event model behind
it. The guiding principle:

> **One event, three projections.** Every log line is a structured event emitted
> once. The human journal line, the machine/agent JSON, and the correlated debug
> trace are all *renderings of the same event*, never independently authored.

This is a **greenfield design**: there are no log consumers yet and no
backward-compatibility constraint, so the current
`crates/temper-runner/src/observability/` scaffolding is mined for good ideas
(stable work-item identity, redaction) and otherwise replaced — see
[Relationship to existing code](#relationship-to-existing-code).

---

## 1. Audiences and what each needs

| Audience | Reads via | Needs |
| --- | --- | --- |
| **Operator** | `journalctl -u temper` | Skimmable, one line per *workflow state change*; a stable tag to follow one work item end-to-end; health at startup. |
| **Agent / machine** | `journalctl -o json`, JSON sink | The same events as typed fields (no English parsing); a reliable join key; numbers as numbers. |
| **Debugger** | `RUST_LOG=temper=debug` | The *causes between* state changes, correlated into a per-item tree, not a flat soup. |

These are not three log systems. They are three filters/renderers over one event
stream. The level policy (§5) and the field schema (§3) are what let a single
emit site serve all three.

---

## 2. Subsystems (the `service` dimension)

Every event names the plane that produced it. These map 1:1 onto the standalone
daemon's assembly (engine + worker + agent on one loop) plus the inbound trigger.

| `service` | tracing `target` | Responsibility | Example events |
| --- | --- | --- | --- |
| `engine`  | `temper::engine`  | Orchestrator: applied transitions, label diffs, queue moves, gate evaluation, merges, resolution. The authoritative state-change log. | `transition.applied`, `gate.evaluated`, `pr.merged`, `item.resolved` |
| `worker`  | `temper::worker`  | Lease lifecycle and "what each role is doing right now": claims, saturation/queue-behind, releases. The concurrency view. | `lease.claimed`, `role.saturated`, `lease.released` |
| `agent`   | `temper::agent`   | The LLM workspace runs (triage, coding): start/finish with duration and a one-line result. The slow, opaque steps. | `agent.started`, `agent.finished` |
| `trigger` | `temper::trigger` | The Forgejo webhook/wake receiver: inbound facts from the outside world. The only lines cross-checkable against Forgejo directly. | `issue.opened`, `wake.received`, `ci.completed` |

The human line is prefixed with the service, padded so the second column aligns:
`engine:  …`, `worker:  …`, `agent:   …`, `trigger: …`.

---

## 3. The event schema (machine-facing contract)

Each event carries a **small closed vocabulary** of fields. The same field names
appear at every site; an agent joins and filters on them without parsing English.

```rust
tracing::info!(
    target: "temper::engine",

    // ---- identity ----
    service       = "engine",             // engine | worker | agent | trigger
    event         = "transition.applied", // closed dotted enum — the machine key

    // ---- correlation (repo-qualified) ----
    repo          = "acme/widgets",
    artifact.kind = "intake",             // intake | code | implementation_pr
    artifact.ref  = "acme/widgets#42",    // THE join key — equals the human tag
    pr.ref        = "acme/widgets PR#44", // only when a PR is involved

    // ---- workflow position ----
    role          = "architect",          // architect | engineer | mechanical
    transition    = "triage_intake_to_code",
    queue.to      = "code_ready",
    labels.delta  = "-untriaged +code +ready",

    // ---- measurements (numbers, not prose) ----
    duration_ms   = 73_000,

    // ---- human projection, last ----
    "engine: [acme/widgets#42] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready"
);
```

### Field rules (the discipline that makes it worth it)

1. **`event` is a closed dotted-namespace enum**, defined in Rust so it cannot
   drift: `issue.opened`, `wake.received`, `lease.claimed`, `lease.released`,
   `lease.lost`, `agent.started`, `agent.finished`, `transition.applied`,
   `queue.entered`, `gate.evaluated`, `pr.opened`, `pr.merged`, `item.resolved`,
   `role.saturated`. An agent keys off `event=…` and never reads prose.
2. **`artifact.ref` is the canonical join key and it equals the human tag.**
   `grep 'acme/widgets#42'` over text and `where artifact.ref == "acme/widgets#42"`
   over JSON return the same set. The two projections share their primary key.
3. **The human message is deliberately redundant with the fields.** Both come
   from one emit site, so they cannot disagree. Agents ignore the message;
   humans ignore the fields; neither path is a second source of truth.
4. **Numbers stay numeric.** `duration_ms = 73000`, never `"1m13s"` in a field.
   The human renderer formats `1m13s`; the machine keeps the integer to threshold
   on.
5. **Dotted, OTel-shaped names** (`artifact.ref`, `queue.to`, `labels.delta`) so
   the later OpenTelemetry export (§6) is a no-op at the emit sites.

### Repo-qualified references (multi-repo)

The join key is repo-qualified — `acme/widgets#42`, `acme/api#7` — because issue
numbers collide across repos. Consequences:

- `grep 'acme/api'` → everything for one repo.
- `grep -E 'acme/api#7|acme/api PR#19'` → one issue and its PR, end-to-end.
- The `transition.applied` line for `open_pr` carries `opened PR #44`; the PR's
  own birth line carries `for #42`; together they make the issue↔PR alias
  discoverable without prior knowledge.

This aligns with ADR 0021 (repo-qualified artifact references).

---

## 4. Spans (what makes machine + debug coherent)

Today events are flat/orphaned. At `info` the `artifact.ref` field re-threads
them; at `debug` that falls apart into hundreds of unparented lines. Two span
layers fix both at zero cost to the emit sites:

```rust
// (a) per-work-item span: opened on lease claim, closed on completion
let item_span = info_span!(
    "work_item",
    repo = %item.repo,
    artifact.ref = %item.refstr,   // inherited by EVERY child event automatically
    role = %role,
    transition = %transition,
);

// (b) per-agent-run span nested inside it
async { /* agent runs */ }
    .instrument(info_span!("agent_run", kind = "coding"))
    .await
```

Payoff:

- **`artifact.ref` is set once on the span and inherited by every child event** —
  including all `debug`/`trace` lines. The §3 join key is automatically present
  on every debug line without per-call ceremony.
- **Debug becomes a tree, not a flood.** Under one `work_item` span you see:
  lease acquisition → feed/queue scan decision → the forge GET that loaded the
  issue → the agent span → each tool call → the applier's forge mutations →
  lease release. All parented, all carrying the same `artifact.ref`.
- **Concurrency stops being confusing.** Items interleave in wall-clock order,
  but each line's span context says which item it belongs to. Filtering one
  `span.id` reconstructs one item's complete trace out of the interleave.

These are OTel spans by construction; `artifact.ref` is a span attribute; the
`event` enum maps to span events.

---

## 5. Level policy

Altitude decides level. The rule that keeps it honest:

> **`info` is a closed vocabulary treated as an API. `debug`/`trace` are open and
> free to change.** Automation and alerts key off `info` events and the `event`
> enum; nobody builds on `debug` strings.

| Level | Contents | Audience |
| --- | --- | --- |
| **info** | One event per *workflow state change* — the §3/§7 catalog. The `event` set is closed and stable. | Operator + agent dashboards. The contract; do not churn. |
| **debug** | Causes *between* state changes: each forge HTTP call (method, path, status, ms); feed/queue-scan decisions ("considered #42 eligible; #43 skipped: role saturated, holder #42"); gate re-evaluations; lease renewals; agent tool-call boundaries; reconcile passes. All inherit the `work_item` span. | Debugging *this daemon* — "why did #43 wait 4 minutes?" |
| **trace** | Wire-level: full request/response bodies, JSON payloads, completion-queue churn, per-poll cadence ticks, raw webhook bodies. | Debugging *temper itself* or a forge-mapping bug. |

Example debug rendering of the `#42`/`#43` contention (every line inherits
`artifact.ref` from its span; the `INFO` line is the operator-facing one):

```text
DEBUG temper::worker  {artifact.ref=acme/widgets#43} scan: candidate eligible, role=architect
DEBUG temper::worker  {artifact.ref=acme/widgets#43} scan: skipped — role architect saturated (1/1), holder=acme/widgets#42
DEBUG temper::engine  {artifact.ref=acme/widgets#42} forge GET /repos/acme/widgets/issues/42 -> 200 (41ms)
DEBUG temper::agent   {artifact.ref=acme/widgets#42 agent_run.kind=triage} tool_call read_file path=src/cache.rs (12ms)
DEBUG temper::engine  {artifact.ref=acme/widgets#42} lease renew ttl=10m (held 47s)
 INFO temper::agent   {artifact.ref=acme/widgets#42} agent: [acme/widgets#42] architect/triage done in 1m13s | verdict=ready_code
```

Filtering is `RUST_LOG` (existing `EnvFilter`): `RUST_LOG=info` (default),
`RUST_LOG=temper::worker=debug,info` (one subsystem), `RUST_LOG=temper=trace`.

---

## 6. Sinks and OpenTelemetry posture

Decided: **journald + JSON toggle now, OTel-shaped, exporter deferred.**

- **Human:** journald (Linux/systemd) or stderr fmt fallback. journald records
  its own timestamps, so message bodies carry **no wall-clock time** — only
  durations the app computes (`done in 1m13s`, `success (4m37s)`,
  `intake -> landed in 9m15s`). Switching `-o short-iso`→`-o short-monotonic`
  changes the time column with nothing redundant in the text. (Already
  implemented in `temper-log`.)
- **Machine:** `journalctl -o json` exposes every §3 field as a JSON key today,
  so under systemd journald already *is* the machine sink. The explicit
  `TEMPER_LOG_FORMAT=json` toggle (implemented in `temper-log`) is for the
  **non-journald** case: it selects a `fmt().json()` layer that writes structured
  JSON lines to stderr regardless of TTY, with span fields included so the
  `artifact.ref` join key threads onto every line. **Zero new emit sites.**

  **Sink precedence** (one sink wins, in order): (1) `TEMPER_LOG_FORMAT=json` →
  JSON on stderr, **even under systemd** — an explicit operator request for
  machine output beats auto-detection; (2) else journald when `JOURNAL_STREAM`
  is set and reachable (the systemd default — and already machine-readable via
  `-o json`); (3) else the human stderr fmt fallback. `RUST_LOG` filtering
  applies to all three.
- **OTel:** adopt the *semantic model* now (spans, dotted attributes, `event`
  enum, `duration_ms`). Do **not** wire an exporter/collector — a single-process
  standalone daemon writing to journald does not need one. `tracing-opentelemetry`
  is present but optional, behind `temper-log`'s **disabled-by-default `otel`
  cargo feature** (`--features otel`); the feature compiles a cfg-gated seam that
  installs an exporter-less OTel layer next to the chosen sink. Shipping to a
  collector (Tempo/Jaeger/Honeycomb) later is *adding one layer next to journald*
  — swap the empty tracer provider for one with a batch/OTLP exporter, no
  emit-site changes. Flip it the day temper goes multi-process/multi-host, which
  is when cross-boundary trace propagation actually pays.

---

## 7. Reference output (the human contract)

The approved operator view, multi-repo with concurrent intake. Two repos
(`acme/widgets`, `acme/api`), three issues filed within seconds; the single
per-role worker (concurrency=1, shared across repos) serializes work, and the
`role.saturated` lines name the cross-repo wait queue.

```text
$ journalctl -u temper -o short-iso

2026-06-16T09:00:01+0000 temper[4821]: engine:  temper 0.9.0 starting | mode=standalone pid=4821
2026-06-16T09:00:01+0000 temper[4821]: engine:  config loaded from /etc/temper/temper.toml
2026-06-16T09:00:01+0000 temper[4821]: engine:  forge: forgejo @ https://git.example.com (reachable, auth ok as temper-bot)
2026-06-16T09:00:01+0000 temper[4821]: engine:  workflow: basic-delivery | roles=architect,engineer,mechanical | queues=5
2026-06-16T09:00:01+0000 temper[4821]: engine:  watching 2 repos: acme/widgets, acme/api
2026-06-16T09:00:01+0000 temper[4821]: engine:  repo acme/widgets: labels verified (untriaged,code,ready,in-progress,implementation)
2026-06-16T09:00:01+0000 temper[4821]: engine:  repo acme/api:     labels verified (untriaged,code,ready,in-progress,implementation)
2026-06-16T09:00:02+0000 temper[4821]: engine:  planes up: engine + worker + agent on one loop
2026-06-16T09:00:02+0000 temper[4821]: trigger: webhook listener up on :8080/forgejo/webhook (issue, PR, CI events)
2026-06-16T09:00:02+0000 temper[4821]: engine:  poll backstop every 60s (architect, engineer feeds across 2 repos)
2026-06-16T09:00:02+0000 temper[4821]: engine:  mechanical backstop every 30s (raw_intake, landing across 2 repos)
2026-06-16T09:00:02+0000 temper[4821]: worker:  capacity: architect=1 engineer=1 mechanical=1 (per-role, shared across all repos)
2026-06-16T09:00:02+0000 temper[4821]: engine:  ready -- watching acme/widgets, acme/api, idle

--------------------------------------------------------------------------------
2026-06-16T10:14:33+0000 temper[4821]: trigger: [acme/widgets#42] issue opened by alice "Cache invalidation drops stale keys on resize"
2026-06-16T10:14:33+0000 temper[4821]: trigger: [acme/widgets#42] wake | artifact=intake queue=raw_intake
2026-06-16T10:14:36+0000 temper[4821]: trigger: [acme/api#7] issue opened by carol "Rate limiter rejects burst within window"
2026-06-16T10:14:36+0000 temper[4821]: trigger: [acme/api#7] wake | artifact=intake queue=raw_intake
2026-06-16T10:14:41+0000 temper[4821]: trigger: [acme/widgets#43] issue opened by bob "Add retry budget to fetch client"
2026-06-16T10:14:41+0000 temper[4821]: trigger: [acme/widgets#43] wake | artifact=intake queue=raw_intake

2026-06-16T10:14:42+0000 temper[4821]: worker:  [acme/widgets#42] mechanical claimed lease (ttl 10m) | running mark_untriaged
2026-06-16T10:14:42+0000 temper[4821]: engine:  [acme/widgets#42] mark_untriaged applied | +untriaged
2026-06-16T10:14:42+0000 temper[4821]: engine:  [acme/widgets#42] -> queue 'triage' | awaiting architect
2026-06-16T10:14:43+0000 temper[4821]: worker:  [acme/api#7] mechanical claimed lease (ttl 10m) | running mark_untriaged
2026-06-16T10:14:43+0000 temper[4821]: engine:  [acme/api#7] mark_untriaged applied | +untriaged
2026-06-16T10:14:43+0000 temper[4821]: engine:  [acme/api#7] -> queue 'triage' | awaiting architect
2026-06-16T10:14:43+0000 temper[4821]: worker:  [acme/widgets#43] mechanical claimed lease (ttl 10m) | running mark_untriaged
2026-06-16T10:14:43+0000 temper[4821]: engine:  [acme/widgets#43] mark_untriaged applied | +untriaged
2026-06-16T10:14:43+0000 temper[4821]: engine:  [acme/widgets#43] -> queue 'triage' | awaiting architect

2026-06-16T10:14:44+0000 temper[4821]: worker:  [acme/widgets#42] architect claimed lease (ttl 10m) | running triage_intake
2026-06-16T10:14:44+0000 temper[4821]: agent:   [acme/widgets#42] architect/triage start | reading issue + repo context
2026-06-16T10:14:44+0000 temper[4821]: worker:  architect busy (concurrency=1) | 2 queued: acme/api#7, acme/widgets#43
2026-06-16T10:15:57+0000 temper[4821]: agent:   [acme/widgets#42] architect/triage done in 1m13s | verdict=ready_code
2026-06-16T10:15:57+0000 temper[4821]: engine:  [acme/widgets#42] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready
2026-06-16T10:15:57+0000 temper[4821]: engine:  [acme/widgets#42] -> queue 'code_ready' | awaiting engineer
2026-06-16T10:15:57+0000 temper[4821]: worker:  [acme/widgets#42] architect lease released

2026-06-16T10:15:58+0000 temper[4821]: worker:  [acme/api#7] architect claimed lease (ttl 10m) | running triage_intake
2026-06-16T10:15:58+0000 temper[4821]: agent:   [acme/api#7] architect/triage start | reading issue + repo context
2026-06-16T10:15:58+0000 temper[4821]: worker:  architect busy (concurrency=1) | 1 queued: acme/widgets#43
2026-06-16T10:15:58+0000 temper[4821]: worker:  [acme/widgets#42] engineer claimed lease (ttl 10m) | running open_pr
2026-06-16T10:15:58+0000 temper[4821]: agent:   [acme/widgets#42] engineer/coding start | preparing workspace, implementing
2026-06-16T10:17:11+0000 temper[4821]: agent:   [acme/api#7] architect/triage done in 1m13s | verdict=ready_code
2026-06-16T10:17:11+0000 temper[4821]: engine:  [acme/api#7] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready
2026-06-16T10:17:11+0000 temper[4821]: engine:  [acme/api#7] -> queue 'code_ready' | awaiting engineer
2026-06-16T10:17:11+0000 temper[4821]: worker:  [acme/api#7] architect lease released
2026-06-16T10:17:11+0000 temper[4821]: worker:  [acme/api#7] engineer busy (concurrency=1) | queued behind acme/widgets#42

2026-06-16T10:17:12+0000 temper[4821]: worker:  [acme/widgets#43] architect claimed lease (ttl 10m) | running triage_intake
2026-06-16T10:17:12+0000 temper[4821]: agent:   [acme/widgets#43] architect/triage start | reading issue + repo context
2026-06-16T10:18:25+0000 temper[4821]: agent:   [acme/widgets#43] architect/triage done in 1m13s | verdict=ready_code
2026-06-16T10:18:25+0000 temper[4821]: engine:  [acme/widgets#43] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready
2026-06-16T10:18:25+0000 temper[4821]: engine:  [acme/widgets#43] -> queue 'code_ready' | awaiting engineer
2026-06-16T10:18:25+0000 temper[4821]: worker:  [acme/widgets#43] architect lease released
2026-06-16T10:18:25+0000 temper[4821]: worker:  [acme/widgets#43] engineer busy (concurrency=1) | queued behind acme/widgets#42, acme/api#7

2026-06-16T10:19:09+0000 temper[4821]: agent:   [acme/widgets#42] engineer/coding done in 3m11s | 4 files changed, +118 -12
2026-06-16T10:19:10+0000 temper[4821]: engine:  [acme/widgets#42] open_pr applied | -ready +in-progress | assignee=engineer | opened PR #44
2026-06-16T10:19:10+0000 temper[4821]: engine:  [acme/widgets PR#44] opened "Fix cache invalidation on resize" | implementation, for #42
2026-06-16T10:19:10+0000 temper[4821]: worker:  [acme/widgets#42] engineer lease released
2026-06-16T10:19:10+0000 temper[4821]: engine:  [acme/widgets PR#44] gates: ci_gate=pending dependency_gate=ok | waiting on CI

2026-06-16T10:19:11+0000 temper[4821]: worker:  [acme/api#7] engineer claimed lease (ttl 10m) | running open_pr
2026-06-16T10:19:11+0000 temper[4821]: agent:   [acme/api#7] engineer/coding start | preparing workspace, implementing
2026-06-16T10:22:33+0000 temper[4821]: agent:   [acme/api#7] engineer/coding done in 3m22s | 3 files changed, +64 -9
2026-06-16T10:22:34+0000 temper[4821]: engine:  [acme/api#7] open_pr applied | -ready +in-progress | assignee=engineer | opened PR #19
2026-06-16T10:22:34+0000 temper[4821]: engine:  [acme/api PR#19] opened "Allow burst within rate-limit window" | implementation, for #7
2026-06-16T10:22:34+0000 temper[4821]: worker:  [acme/api#7] engineer lease released
2026-06-16T10:22:34+0000 temper[4821]: engine:  [acme/api PR#19] gates: ci_gate=pending dependency_gate=ok | waiting on CI

2026-06-16T10:22:35+0000 temper[4821]: worker:  [acme/widgets#43] engineer claimed lease (ttl 10m) | running open_pr
2026-06-16T10:22:35+0000 temper[4821]: agent:   [acme/widgets#43] engineer/coding start | preparing workspace, implementing
2026-06-16T10:23:50+0000 temper[4821]: trigger: [acme/widgets PR#44] CI completed: success (4m40s)
2026-06-16T10:23:50+0000 temper[4821]: engine:  [acme/widgets PR#44] gates: ci_gate=ok dependency_gate=ok | -> queue 'landing' eligible to land
2026-06-16T10:23:51+0000 temper[4821]: worker:  [acme/widgets PR#44] mechanical claimed lease (ttl 10m) | running land_pr
2026-06-16T10:23:52+0000 temper[4821]: engine:  [acme/widgets PR#44] merged -> main (squash e3f9a1c)
2026-06-16T10:23:52+0000 temper[4821]: engine:  [acme/widgets#42] resolved -- implemented by PR#44 | intake -> landed in 9m19s
2026-06-16T10:23:52+0000 temper[4821]: worker:  [acme/widgets PR#44] mechanical lease released

2026-06-16T10:25:48+0000 temper[4821]: agent:   [acme/widgets#43] engineer/coding done in 3m13s | 2 files changed, +47 -3
2026-06-16T10:25:49+0000 temper[4821]: engine:  [acme/widgets#43] open_pr applied | -ready +in-progress | assignee=engineer | opened PR #45
2026-06-16T10:25:49+0000 temper[4821]: engine:  [acme/widgets PR#45] opened "Add retry budget to fetch client" | implementation, for #43
2026-06-16T10:25:49+0000 temper[4821]: worker:  [acme/widgets#43] engineer lease released
2026-06-16T10:25:49+0000 temper[4821]: engine:  [acme/widgets PR#45] gates: ci_gate=pending dependency_gate=ok | waiting on CI

2026-06-16T10:26:58+0000 temper[4821]: trigger: [acme/api PR#19] CI completed: success (4m24s)
2026-06-16T10:26:58+0000 temper[4821]: engine:  [acme/api PR#19] gates: ci_gate=ok dependency_gate=ok | -> queue 'landing' eligible to land
2026-06-16T10:26:59+0000 temper[4821]: worker:  [acme/api PR#19] mechanical claimed lease (ttl 10m) | running land_pr
2026-06-16T10:27:00+0000 temper[4821]: engine:  [acme/api PR#19] merged -> main (squash 7b2c004)
2026-06-16T10:27:00+0000 temper[4821]: engine:  [acme/api#7] resolved -- implemented by PR#19 | intake -> landed in 12m24s
2026-06-16T10:27:00+0000 temper[4821]: worker:  [acme/api PR#19] mechanical lease released

2026-06-16T10:30:12+0000 temper[4821]: trigger: [acme/widgets PR#45] CI completed: success (4m23s)
2026-06-16T10:30:12+0000 temper[4821]: engine:  [acme/widgets PR#45] gates: ci_gate=ok dependency_gate=ok | -> queue 'landing' eligible to land
2026-06-16T10:30:13+0000 temper[4821]: worker:  [acme/widgets PR#45] mechanical claimed lease (ttl 10m) | running land_pr
2026-06-16T10:30:14+0000 temper[4821]: engine:  [acme/widgets PR#45] merged -> main (squash 9d1ef52)
2026-06-16T10:30:14+0000 temper[4821]: engine:  [acme/widgets#43] resolved -- implemented by PR#45 | intake -> landed in 15m33s
2026-06-16T10:30:14+0000 temper[4821]: worker:  [acme/widgets PR#45] mechanical lease released

2026-06-16T10:30:14+0000 temper[4821]: engine:  idle -- watching acme/widgets, acme/api
```

### Format rules

- **Pure ASCII.** `->` for transitions/queue moves, `--` for separators/em-dash,
  `|` as field separator, `[repo#n]` / `[repo PR#n]` as correlation tags. Survives
  `grep`, `-o cat`, and non-UTF terminals.
- **Service prefix on every line**, padded so the message column aligns.
- **Concurrency is first-class:** `role.saturated` lines name the cross-repo wait
  queue; the startup `capacity:` line states per-role concurrency so later
  "queued behind" lines make sense.
- **Global `role.saturated` lines** (about the shared resource) carry no subject
  `[repo#n]` tag but name the waiting items so they stay greppable.

---

## 8. Relationship to existing code

This is greenfield: **there are no log consumers yet, no backward-compatibility
constraint.** Anything currently in the tree may be changed or deleted. The
existing `crates/temper-runner/src/observability/` is treated as prior art to
mine for good ideas, not a contract to preserve.

What it got right, and we keep the *ideas* (re-homed, not the API):

- A stable, provider-neutral work-item identity (`WorkItemIdentity`) — the
  concept behind `artifact.ref` and the `work_item` span's identity.
- Typed per-event structs instead of stringly emit sites.
- Bounded, redacted field rendering (`redact.rs`) — preserved as the redaction
  rule for free-text fields (issue titles, agent summaries).

What it got wrong, and we **remove outright:**

- `render_*` produces a JSON **string** stuffed into the tracing **message**
  (`tracing::info!(target: "worker", "worker: {}", render_scan_summary_event(...))`).
  This traps machine fields inside a string: a JSON subscriber sees opaque text,
  a human reads JSON, and nothing can filter on a field. **This whole
  `render() -> String`-in-message path is deleted**, along with the
  `StructuredEvent` string builder.

The replacement is the core of this design — fields become **real tracing
fields**, the human message is rendered separately, both from one `emit()` site:

```rust
tracing::info!(
    target: "temper::worker",
    service = "worker",
    event = "scan.summary",
    repo = %ev.repo,
    role = ev.role,
    work_item_count = ev.work_item_count,
    "worker: scan {repo} role={role} -> {count} items",   // human, generated here
);
```

Because there is no consumer to migrate, we do not phase this in: the new event
model in `temper-log` is the single source, the runner's `observability/`
string-rendering is removed, and call sites switch to `emit()` in one pass.
Net direction: **identities and redaction survive (re-homed into the event
model); the string-in-message machinery is thrown away.**

### Ownership / where the code lives

- **`temper-log`** owns the event model: `Service`, the closed `Event` enum, the
  `WorkItemRef` formatting, the human-duration formatter, the `emit()`
  constructors, the span helpers, redaction, and the sink wiring (journald /
  stderr / JSON toggle / future OTel feature). It already owns `init_logging`.
- **`crates/temper-runner/src/observability/`** shrinks to the runner-specific
  glue that *builds* event inputs from runner types (workflow ids, scan results)
  and calls `temper-log`'s `emit()`. The JSON-string renderers and
  `StructuredEvent` are deleted.

---

## 9. Implementation order (smallest first)

No consumer exists, so this is a clean build, not a phased migration. The order
below is purely to keep each PR reviewable.

1. **Event model in `temper-log`:** `Service` enum, the closed `Event` enum (the
   full dotted catalog from §3), `WorkItemRef` (repo-qualified `artifact.ref`
   formatting), the human-duration formatter (`1m13s`), and redaction for
   free-text fields. Define the `emit()` constructors (one per `Event` variant)
   that expand to `tracing::*!` with real fields + the generated human message.
2. **Delete the old path and switch call sites:** remove the runner's
   `render_*` → string-in-message functions and `StructuredEvent`; convert every
   emit site to a `temper-log` `emit()` call. Add the events that didn't exist
   before (`lease.*`, `gate.evaluated`, `pr.merged`, `item.resolved`,
   `role.saturated`). One pass — no compatibility shim.
3. **Open the two spans** (`work_item`, `agent_run`) at the lease-claim/agent
   boundaries; set `artifact.ref` on the span so every debug line inherits it.
4. **Level policy pass:** put forge-HTTP / scan-decision / lease-renew /
   tool-call logs at `debug`, leaving only the §7 catalog at `info`.
5. **JSON sink toggle** (`TEMPER_LOG_FORMAT=json` → `fmt().json()`); document
   `journalctl -o json` for the journald path.
6. **OTel seam:** add `tracing-opentelemetry` behind a disabled feature; no
   emit-site changes when it flips on.

Steps 1–2 establish the schema and remove the broken string-in-message path; 3
delivers the debug tree and auto-threads the join key; 4 settles levels; 5–6 are
output toggles with no new emit sites.
