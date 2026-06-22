# Basic-delivery observability guide

Use this page while `./run.sh start` is running, or after teardown against the
retained logs. `./run.sh stop` removes throwaway Forgejo state and credentials;
logs stay under `logs/`.

The Temper topology is one process: `temper serve standalone`. The engine,
in-process worker, and coding agent all write to `logs/run.log`. Engine lines
are prefixed `engine:`; worker lines are prefixed `worker:`.

## Where to look

- `logs/provision.log` — init artifacts, webhook registration, initial commit,
  and the seeded site-admin intake issue URL.
- `logs/run.log` — standalone readiness, accepted webhook deliveries, wake
  scans, job assignment/result lines, worker assignment/result lines, and
  mechanical automation/landing.
- `logs/repo-populate.log` — the explicit first commit that installs the tiny
  project README and bundled pass-through CI.
- `logs/runner.log` — Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from throwaway Forgejo.

## Minimal movement trail

For the fixed single-repo run, expect interleaved lines like:

```text
worker: registered worker_id=temper-worker-1 capabilities=2
engine: webhook accepted repo=acme/service kind=Issue item=<n>
engine: webhook wake scan repo=acme/service enqueued=1
engine: assigned job_id=... role=architect repo=acme/service worker=temper-worker-1
worker: assigned job_id=... role=architect repo=acme/service
worker: result sent job_id=... status=success
engine: result received job_id=... status=success disposition=...
engine: assigned job_id=... role=engineer repo=acme/service worker=temper-worker-1
worker: result sent job_id=... status=success
engine: ... mechanical landing ...
```

The seed-last webhook-wake proof is the `webhook accepted` → `webhook wake scan …
enqueued=1` pair. The intake issue is filed only after standalone readiness, so
its creation webhook drives the first scan instead of the slow poll backstop.
