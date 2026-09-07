# Graph artifact experiment

This protocol belongs to Temper issue #1281, under plan #1278. The checked-in
configuration and harness identities are frozen before native comparisons.
The recorded run uses the accepted shared-daemon implementation from #1280.
The benchmark does not enable persistence in Temper's default configuration.
The [freeze record](freeze.json) pins the harness commit, module checksums and
exact configuration and protocol bytes before any native observation.

## Inputs and correctness

The source dataset is the complete Temper repository at these two commits:

| Revision | Commit | Controlled probe state |
| --- | --- | --- |
| P0 | `386d70b28297cd20a40dd2ad90b5d0a0124d1f8a` | Producer baseline |
| P1 | `b7a7d9140b359baf0aa111c25af43cb67173764d` | Add, edit, delete, rename |

The small independent Rust crate in
`benchmarks/codebase-memory-artifact-probes/` makes those mutations explicit.
P1 adds `artifact_probe_added`, changes `artifact_probe_edited`, deletes
`artifact_probe_deleted`, and moves `artifact_probe_renamed` to a new file.
Both revisions also query the unchanged production `read_initialize` and
`retry_read` functions. The configuration pins every expected source range
and its SHA-256, four call relationships, and the corresponding coverage paths.

Readiness requires all symbol, exact local source, call, negative-path,
current-root and coverage-generation checks to pass. The consumer checkout
differs from the producer checkout. Index status and coverage bracket the
query set. Coverage remains best effort; these brackets are not an atomic
snapshot. A ready response or matching node count alone cannot pass the gate.

The provider is version 0.10.8, executable SHA-256
`cb6dc545a8cb714799461ba7d8c223dbca7d02ca46ea5a4215c1beb450fd8e09`.
Index mode is `full`. Configured automatic indexing, watching and UI are off,
with public CLI readback before MCP sessions. Analysis readers and an explicit
`all` mutation client use the same isolated account and canonical cache root.
Each indexing request has an explicit checkout working directory.

Clean independent clones use no hardlinks, an empty Git template and external
exclude policy, and pinned tracked ignore files. Configuration records that
identity. No operator cache or session is used. Linux UID/GID 62078 is reserved
for this run and must have no processes before admission. State belongs to a
new private task directory. Application caches are controlled; OS page caches
are not flushed. Hardware, filesystem and tool versions accompany the results.

## Predeclared order

One clean P0 producer intentionally exports with `persistence: true` before
the consumer matrix. No watcher export is used. Five repetitions contain
these five matched cells, in the following fixed balanced order:

| Repetition | First | Second | Third | Fourth | Fifth |
| --- | --- | --- | --- | --- | --- |
| 1 | cold-P0 | artifact-P0 | cold-P1 | stale-P1 | warm-P1 |
| 2 | artifact-P0 | cold-P1 | stale-P1 | warm-P1 | cold-P0 |
| 3 | cold-P1 | stale-P1 | warm-P1 | cold-P0 | artifact-P0 |
| 4 | stale-P1 | warm-P1 | cold-P0 | artifact-P0 | cold-P1 |
| 5 | warm-P1 | cold-P0 | artifact-P0 | cold-P1 | stale-P1 |

Cold cells have no local database and no artifact. Artifact cells import the
P0 export with no local database. Stale cells consume that export at P1.
Warm cells first index P0 without persistence, then advance the same checkout
and retained daemon/database to P1. Preparation is reported separately.
All other cells begin with a cold daemon. Only disposable task databases may
be cleared between cells, after the entire account has exited naturally.

Missing, corrupt, truncated and incompatible-schema artifacts each have one
additional observation after the matrix, in that order. Each records the
harness refusal and a separate raw-provider experiment on disposable state.
The malformed bytes are quarantined before a fresh-project recovery index.
The raw-provider experiment is not a production download-validation path.
Schema mutation does not establish compatibility with a future executable.

## Metrics and decision rule

The primary clock begins after checkout, before transfer, artifact validation
and provider startup, and ends only when the fixed query set is correct.
It includes failed import or incremental work and any safe rebuild. Consumer
job time includes checkout through natural account shutdown. One-time global
configuration and producer/export costs are separate; warm P0 preparation is
excluded from P1 consumer time and reported explicitly.
Job time is local benchmark wall time; CI queueing and runner provisioning are
not measured by this experiment.

Each provider request has a 1,200-second deadline. Natural account shutdown
has a 45-second deadline. A timeout, unavailable result, source mismatch or
forced cleanup remains a failed observation. All samples, including failures,
are retained. Failed cells have no success-only performance median. No outlier
removal or selective sample replacement is allowed. A harness defect may
require a versioned rerun; the aborted attempt and reason remain documented,
and the threshold and query contract do not change after observations.

The report gives all samples and median/minimum/maximum for complete cells.
It records request counts, combined request durations, local-copy transfer,
startup, fallback, database/cache/artifact bytes, and sampled concurrent RSS
across the account's native processes. Inclusive subprocess CPU includes the
daemon and indexing workers; component CPU is not inferred from frontend CPU.
Sampling at 20 ms can miss brief memory peaks. Import, compression and
incremental times are not separated unless a reliable observable boundary
exists. Producer and inspection costs are not silently attributed to import.

A bounded CI pilot is a candidate only when all 25 main observations and four
failure recoveries are correct and exit naturally, every main observation
succeeds without an initial refusal, and both matched artifact comparisons
improve median ready-and-correct time by at least 20% with no median consumer
job regression. Same-source P0 and P1 baselines are compared separately.
Recovered foreign-root imports cannot qualify as clean artifact success.

The 20% floor requires a useful reduction beyond small local timing variation.
Producer cost, storage and observed consumer savings determine the reported
fanout break-even. Local copy does not measure a remote CI download. Passing
these gates can justify only a bounded opt-in CI pilot; broad adoption also
requires measured transport and operational ownership. Otherwise defer.

Git distribution is not piloted in this experiment. Its clone/history costs
are estimated from measured artifact bytes, with compression assumptions
stated. A later Git pilot must test fresh-clone and real Forgejo server merge
behavior: an attribute naming a custom driver does not install its config.
Immutable CI artifacts and retained local caches are compared operationally.

Only the sanitized report, manifest facts and input identities are published.
Raw SQLite files, source properties, absolute runtime paths, prompts,
credentials and diagnostics remain private. The mapped Jig scenario proves
the same harness's validation and delivery workflow using an explicitly
synthetic adapter. Its timings cannot establish native performance.
