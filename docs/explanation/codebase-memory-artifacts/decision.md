# Defer shared graph artifacts

The pinned 0.10.8 experiment does not justify shared artifact adoption.
Same-commit artifacts took 50.0% longer to reach correct results than cold
indexing, and stale artifacts took 10.2% longer than their matched cold
baseline. Neither meets the predeclared 20% improvement threshold; both also
regress median consumer job time. Keep Temper's existing opt-in behavior.

The [protocol](experiment.md), [configuration](config.json),
[corrected harness freeze](freeze-r2.json), and [host facts](host.json) identify
the inputs. [All native observations](results.json) retain every main and failure
sample, resources, storage, producer costs and final decision. The independent
[evidence audit](audit.json) recomputes summaries, joins private observations and
records failure-input provenance and post-probe identities. The report SHA-256
is `ea70a926e5de009313a14b6bae6b16137fbd789634376d5b3741b6433bb2fcf3`. The complete r2 job took 1,685.07 seconds
(28.08 minutes), including producer setup, warm preparations and failure probes.
The original [freeze](freeze.json) predates any measurements.
[Attempt r1](attempt-r1.json) retains the interrupted first run: an observer
bug erased process roles at exit and used the wrong daemon argument. Its
samples are excluded as a whole, with no performance conclusion. The corrected
r2 run repeats the entire original order; source, queries and thresholds did
not change.

## Main results

Each row contains five observations. Ready means the complete declared query
set passes at the current consumer root, including exact source bytes, calls,
coverage-generation brackets and deletion/rename checks. All 25 main samples
eventually passed and all their native sessions exited naturally. The artifact
P0 row includes its initial refusal and necessary rebuild.

| Cell | Ready median, seconds (min–max) | Job median, seconds | CPU median, seconds | Maximum sampled RSS, GiB |
| --- | --- | --- | --- | --- |
| Cold P0 | 38.88 (37.65–42.24) | 40.56 | 151.29 | 2.815 |
| Artifact P0 | 58.33 (55.93–60.36) | 60.27 | 170.58 | 2.823 |
| Cold P1 | 39.52 (38.06–40.13) | 41.16 | 151.58 | 2.817 |
| Stale artifact P1 | 43.54 (41.67–46.11) | 45.12 | 154.39 | 2.813 |
| Retained warm P1 | 31.97 (31.62–33.60) | 32.55 | 143.73 | 2.816 |

CPU is inclusive measured-session subprocess usage, including daemon and
workers, summed across cores. RSS is the largest sampled concurrent account
sum. These are measurements on one Linux host, with OS caches left intact,
not a fleet-wide latency or memory guarantee.

All five same-commit artifact candidates initially reported the producer's
root. The harness refused them before reading snippets, then indexed a fresh
project at the consumer root. Each required two index requests. Median fallback
time was 38.31 seconds (38.22–42.49). A manifest passing its checksum gate did
not authorize the incorrectly bound graph.

All ten main artifact consumers also refreshed their local archive despite
`persistence=false`. The five P0 outputs were identical 26,524,580-byte archives;
metadata export timestamps advanced while public status and the retained local
database still identified the producer root and original generation. Stale P1
outputs identified P1 and their current consumer roots. An export timestamp is
not current-source evidence. The audit records archive checksums separately
from root facts obtained from public status and retained local databases.

All five stale-artifact candidates passed directly after the controlled
add/edit/delete/rename changes, with one index request each, but remained slower
than cold P1 indexing. This does not establish correctness for every kind of
incremental change. Public responses do not provide an independent successful
import timer; the observed index phase includes bootstrap and update work.

Retained warm P1 uses the same checkout and daemon as its P0 preparation.
Its ready median was 19.1% lower and its job median 20.9% lower than cold P1.
P0 preparation cost a separate median 41.11 seconds (38.93–43.20), with its CPU,
memory and index request recorded separately. This does not test retained-cache
reuse across arbitrary new checkout roots.

## Failure and upgrade observations

All four isolated failure observations were refused by the manifest gate and
recovered at the consumer root after quarantining the post-probe directory. Each retained a separate raw
provider probe before wrapper recovery; every native session exited naturally.
These single failure observations establish behavior, not a latency distribution.

| Injected case | Initial refusal | Probe plus recovery ready, seconds | Wrapper fallback, seconds |
| --- | --- | --- | --- |
| Missing | artifact-missing | 76.13 | 38.35 |
| Corrupt | artifact-checksum-mismatch | 81.91 | 38.10 |
| Truncated | artifact-size-mismatch | 81.30 | 40.74 |
| Incompatible schema | artifact-incompatible-schema | 77.04 | 39.50 |

The raw installed-provider probes all returned `indexed` and passed the fixed
query set without an explicit artifact failure response. They do not expose a
stable import-success signal. The wrapper keeps the original artifact refusal
separate from valid query results and never treats a native success as artifact
compatibility evidence. Each row includes two index requests: the disposable
probe and the explicit artifact-free recovery. Failure transfer bytes describe
payload remaining after local fault injection, not remote wire costs.

The corrupt and truncated raw probes refreshed the existing artifact during
indexing despite `persistence=false`: quarantine contains newly written P1
schema-2 artifacts, not the original malformed bytes. The frozen protocol's
phrase "malformed bytes are quarantined" did not predict this provider behavior.
The original inputs can be reconstructed from the immutable producer archive
and frozen deterministic mutation, but are not preserved as quarantine output.
Probe costs include this refresh. Missing and incompatible inputs were unchanged.
This observation further limits consumer-write isolation for any future pilot.

The incompatible case mutates the schema marker. It proves current rejection
and recovery, not compatibility with a future executable or database schema.
A provider upgrade requires a newly pinned producer/build and complete rerun.

## Storage and producer cost

One clean P0 producer intentionally exported with persistence enabled. Its
index/export/correctness phase took 27.43 seconds; one-time configuration took
12.06 seconds and artifact inspection 2.12 seconds. Export/compression has no
independent public timer, so those operations are not assigned invented costs.
Startup totals overlap index phases and must not be added to them.

The archive is 16,942,139 bytes (16.16 MiB), with SHA-256
`876e57806a1bf3617452f663aa933c50e95bd01a765a2f441ede133450ea208e`.
Its compacted database expands to 109.50 MiB. Median local database size was
165.44 MiB for cold, stale and retained-warm cells. Same-commit recovery retained
both the refused and rebuilt databases, totaling a median 343.38 MiB.

Local artifact copies took about 6 ms. Remote CI download was not measured.
Since neither artifact comparison saves consumer time, this run has no positive
producer-fanout break-even. One hundred distinct archives would add about
1.58 GiB of compressed payload before any useful Git delta savings; this is an
estimate, not a measured repository pack or clone cost.

SQLite inspection passed integrity and project identity checks. It found
2,192 absolute-path-bearing values and 18,408 values with source-property keys.
These counts describe the inspection heuristic, not an exhaustive privacy
audit. The database retains checkout paths and source-derived data. Only
sanitized facts and checksums belong in the public report; artifact access
must follow repository source access.

## Operational consequence

Defer both Git-committed artifacts and a CI artifact rollout. Git distribution
was not piloted, so neither fresh-clone artifact behavior nor real Forgejo
custom-driver merge behavior is claimed. The
[distribution comparison](distribution.md) covers history/download costs,
producer ownership, freshness, provider upgrades, fallback and rollback.

A future proposal must resolve same-commit current-root binding and repeat the
fixed correctness/performance protocol with the intended provider build and
real transport. Repository maintainers own producer acceptance; runner
operators own account/cache admission. Consumers must not publish replacement
shared artifacts or bypass maintenance identity checks. No default persistence,
shared producer or production cache-deletion policy changes in this work.
