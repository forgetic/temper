# Forgejo v16 Actions API CI backfill

This active, provisional scenario is the focused historical mapping for feature
`ai/temper#766`, plan `ai/temper#767`, at the canonical source branch
`agent/pr-for-feature-766`.

The bundle inherits the basic-delivery topology and workflow shape, but owns its
Jig script, repository seed, and CI workflow. Its real host-mode Forgejo Actions
run has exactly two jobs: one succeeds and one intentionally exits unsuccessfully.
The Forgejo v16 job response exposes that failed job as a status-only `failure`,
which Temper must retain as provider conclusion `failure` while mapping the
portable conclusion conservatively to `Unknown`.

## Proof contract

The generic `implementation-pr-terminal-ci` convergence strategy stops after one
implementation PR has a complete provider CI snapshot without requiring that CI
to be landable. The manifest then asserts declaratively that:

- one provider run materializes exactly two jobs at the implementation PR head;
- two observations retain stable job, run, attempt, and commit identities;
- exactly one terminal job is `Success` and one is `Unknown` with provider
  conclusion `failure`;
- token-authenticated JSON GETs read the Actions runs collection and that run's
  jobs endpoint; and
- no legacy tasks, login/UI, repository Actions live-view, or Actions mutation
  routes were retained.

The bounded request recorder stores only method, path, query-key names, JSON
acceptance, and authentication scheme/presence. It never stores token values or
unrelated headers, and required assertions fail closed when observations are
missing or request capture drops records.

## Running

From the repository root:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/forgejo-v16-api-ci
./.temper/pre-pr
```

The live run provisions disposable Forgejo v16, starts the real host runner and
standalone Temper, and leaves runtime evidence only in the caller-owned artifact
workspace.
