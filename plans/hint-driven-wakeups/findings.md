# Hint-driven wakeups findings

## Phase 5 — reference-delivery long-poll smoke (2026-06-01)

Validation host had the pinned Forgejo 7.0.12 and `forgejo-runner` 3.5.1
binaries cached under `.cache/forgejo/`, plus ChatGPT OAuth credentials in the
shared pi auth file. I rebuilt the production binaries first so the launcher used
the new worker/trigger diagnostics:

```sh
cargo build --release -p temper-production
cd examples/reference-delivery
POLL_MS=120000 RUN_SECS=300 TEMPER_SKIP_BUILD=1 nohup ./run.sh >logs/driver.log 2>&1 &
./run.sh validate-webhooks
./run.sh stop
```

`validate-webhooks` passed after about 25 seconds, well before the 120 second
poll backstop:

```text
trigger summary: accepted=16 sent_batches=16 no_socket_batches=0 send_failures=0
worker architect.log: consumed_wake=yes wake_tick=yes
worker engineer.log: consumed_wake=yes wake_tick=yes
worker human.log: consumed_wake=yes wake_tick=yes
worker mechanical.log: consumed_wake=yes wake_tick=yes
worker owner.log: consumed_wake=yes wake_tick=yes
worker reviewer.log: consumed_wake=yes wake_tick=yes
worker summary: workers=6 consumed=6 wake_ticks=6 wake_progress=1 wake_no_work=6
webhook wake validation passed
```

Findings:

- Waiting for trigger readiness via `logs/trigger.log` worked before provisioning
  registered the hook.
- Launching non-architect role workers plus mechanical first, waiting for their
  sockets, and launching architect last avoided the first handoff
  `no_sockets` race.
- Broad wake delivery is visible: every worker consumed at least one wake; only
  the worker with active queue work reported wake-triggered `actions>0`, while
  others logged wake-triggered `actions=0` no-work ticks.
- A first attempt before rebuilding `target/release` used stale binaries and
  produced the older log format. Operators who set `TEMPER_SKIP_BUILD=1` must
  ensure the production binaries are current.

Generated `examples/reference-delivery/logs/`, `run/`, and local secret files
were removed after recording this summary.
