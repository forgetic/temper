# Lesson 0015: Start downstream wake sockets before seeded work can hand off

## Tags

`webhook`, `process`, `forgejo`, `testing`

## Trigger

Phase 5 of `plans/hint-driven-wakeups/` made the operator demo support
`POLL_MS=120000`. The seeded intake issue is present before workers start, so the
architect can process it immediately on its first tick.

## What went wrong

If the architect worker starts first, it can transition the intake issue before
engineer/reviewer/owner/mechanical workers have bound their Unix wake sockets.
The trigger may accept the webhook but find only a subset of sockets, so the
next worker sleeps until the long poll backstop even though webhooks are working.

## Steering for future agents

For long-poll webhook demos/tests, start trigger first, then start downstream
workers and wait for their wake sockets, and only then allow the worker that can
perform the first handoff to run. If the initial work is already seeded, launch
that first-handoff worker last or seed only after every worker has completed a
no-work readiness tick.

## Where this is now documented

- `examples/reference-delivery/run.sh` launches non-architect role workers and
  the mechanical worker first, waits for their sockets, then launches architect.
- `examples/reference-delivery/README.md` troubleshooting explains persistent
  `wake_delivery outcome=no_sockets` as a worker startup/socket race.
