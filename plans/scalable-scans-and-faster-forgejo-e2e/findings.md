# Findings — scalable scans and faster Forgejo e2e

## Observed Forgejo multiprocess timing

Warm local measurements from the phase work:

| Topology | Test time | Wall time | Notes |
| --- | ---: | ---: | --- |
| Legacy per-scenario server+runner | 119.69s | 122.10s | Every scenario paid ~1.7–5.3s server startup, ~0.08–0.12s runner registration, and ~4.0–4.3s full provisioning before convergence. |
| Shared server+runner, fresh repos | 93.27s | 93.37s | Shared setup: 1.93s server, 0.08s runner, 3.12s role identities. Per-scenario repo provisioning: ~0.86–0.92s single repo, 2.05s cross-repo. |
| Shared topology follow-up runs | 92.85–98.91s | — | Worker convergence and real CI remained dominant. |
| Phase 6 validation run | 94.15s | — | `cargo test -p temper-testing -- --ignored --test-threads=1` subtest time for `forgejo_multiprocess`. |

Net improvement on this host: about 21–27s test time, roughly 17–22%.

## Scan acceptance findings

- Normal role scans derive list queries from that role's subscribed queues.
- Open artifacts are listed by state; closed/merged artifacts are listed only
  with non-empty workflow/queue labels.
- Candidate list calls request `ItemListDetails::summary()`, avoiding dependency
  N+1 reads until dependency-gated queues or transitions reload exact targets.
- CI, review, and dependency signals are read lazily after cheap queue matching.
  Non-CI role scans perform zero `list_ci_jobs` calls in the default regression
  suite.
- Forgejo labelled PR scans use the issue label index (`type=pulls`, `state`,
  `labels`) plus exact `/pulls/{number}` fetches for matches, not broad
  `/pulls?state=all` scans.
- Immediate role-worker wake ticks narrow to configured repositories named by
  known hints. Unknown/no-hint wakes and poll/audit ticks remain safe broad
  backstops. Production mechanical wake ticks stay broad to preserve cross-repo
  recovery.

## Diagnostics added

The ignored Forgejo multiprocess suite prints per-scenario timing plus worker
scan summaries. It sets `TEMPER_FORGEJO_CI_DIAGNOSTICS=1`, so each worker summary
includes tick count, summed scanned repositories, web-UI CI-read log line count,
and the last scanned repository paths. Timeout panics include those summaries
together with worker log tails, runner log tail, and per-repo CI diagnostics.
