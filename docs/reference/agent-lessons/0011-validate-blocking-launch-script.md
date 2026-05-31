# Lesson 0011: Validate the blocking launch script in the background, then stop via its sentinel

## Tags

`process`, `tooling`, `forgejo`, `testing`

## Trigger

Validating `examples/reference-delivery/run.sh` end-to-end (plan
`real-world-example`, phase B4): the script is the operator entry point, so it
had to actually run, converge, and tear down — not just `sh -n`.

## What went wrong

`./run.sh start` (the default) boots every process and then **blocks** in a
`monitor` loop until a stop-file, the server dying, or `RUN_SECS`. Invoking it
directly in a non-interactive agent shell hangs the turn. Naively `kill`ing the
driver also risks orphaned `forgejo`/`forgejo-runner`/worker processes (lesson
0009) because a hard kill skips the EXIT trap.

## Steering for future agents

To validate a blocking launcher non-interactively:

- Run it backgrounded (`nohup ./run.sh start >log 2>&1 &`), optionally with a
  shorter `RUN_SECS`, and poll the Forge API for the convergence state (issue
  labels, PR merged + reconciled labels) rather than scraping stdout.
- Tear down with `./run.sh stop`, which kills via the saved PID files
  independently of the blocking driver, then **verify**: no orphan
  `forgejo`/`harness-testing-worker` processes (`pgrep -af`), the port is freed,
  and `run/` is removed. `secrets/roles.env` is gitignored — confirm with
  `git check-ignore` and remove it after the run.
- Use the default auth (ChatGPT OAuth, `~/.pi/agent/auth.json`) — never bill
  DeepSeek for a validation run (the cost policy).

## Where this is now documented

`examples/reference-delivery/README.md` (Quick start / Teardown),
`examples/reference-delivery/run.sh` (orphan-cleanup header + `cleanup`),
`plans/real-world-example/findings.md` (B4 run), and lesson 0009 (CPU/orphans).
