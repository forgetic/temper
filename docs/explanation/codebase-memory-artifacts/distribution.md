# Graph artifact distribution and ownership

The [experiment](experiment.md) measures a local filesystem transfer. It does
not measure remote download, Git packing or server merge behavior. Those
limits apply even if its native timing threshold passes.

Let `A` be the measured compressed artifact plus manifest bytes, `P` the
producer job cost, and `S` the matched median consumer saving after transfer,
validation and cleanup. With positive `S`, the time break-even is at least
`ceil(P / S)` consumers per published generation. A remote download adds its
observed cost before recalculating `S`; local copy cannot substitute for it.
Retained-cache preparation is accounted separately in the warm cell.

| Distribution | Consumer cost | Producer/storage cost | Operational requirement |
| --- | --- | --- | --- |
| Git commit | Artifact objects join clone/fetch; import and validation still run | Every distinct retained export grows history | Contributor and actual Forgejo merge behavior must be proven |
| Immutable CI artifact | Download by exact source/build/config identity, verify, import | One intentional producer; retention can expire old generations | Repository access controls and immutable artifact naming |
| Retained local cache | No shared-artifact download; incremental update | Initial index plus account-local database storage | Existing session admission, current-root checks and safe maintenance |

## Git feasibility

No Git artifact pilot is performed here. The producer's local source clone
is benchmark setup, not a test of cloning a repository-committed graph.
Consequently no server merge claim is made.

For `k` independent exports, `k * A` estimates added object payload when Git
cannot obtain useful deltas between compressed snapshots. This is a sizing
estimate, not a measured pack size or a guaranteed bound on total repository
size. Full clones retain history; shallow clones change that cost but do not
solve source identity, refresh or server merge requirements.

The audited provider writes a `.gitattributes` rule naming `merge=ours` and
best-effort producer-local `merge.ours.driver=true` configuration. Git defines
custom drivers in repository or user configuration, separately from
attributes. A clone does not inherit the producer's `.git/config`.
See the [Git custom-driver documentation](https://git-scm.com/docs/gitattributes#_defining_a_custom_merge_driver)
and the [audited export implementation](https://github.com/DeusData/codebase-memory-mcp/blob/v0.10.8/src/pipeline/artifact.c).

A future Git pilot must exercise a fresh contributor clone, conflicting
artifact updates and a real Forgejo server merge. Configuring only a local
test clone cannot establish the server's behavior. Picking one generated
database during merge also does not prove it describes merged source; the
accepted source must be reindexed and a replacement artifact published.

Git introduces a second identity distinction: the source commit used to
produce an artifact differs from the later commit that contains that artifact.
A pilot must explicitly validate that relationship and the generated-path
ignore policy. It must not silently accept any ancestor artifact as current.

## A bounded CI pilot, if the evidence supports one

Repository maintainers would own the producer and acceptance criteria. Runner
operators would own the dedicated account, cache admission and retention.
Consumers would have read access to shared artifacts and would never overwrite
the shared generation from their ephemeral checkout.

Use one intentional producer on clean accepted main. Its immutable key must
include source commit, executable SHA-256, index mode/configuration identity,
ignore-policy identity and artifact schema. Publish only after the producer's
current-root/source/call/coverage gates, natural cleanup, manifest checksum
verification and retained-data inspection pass. Record export and upload cost.

Give the artifact the repository's source-access restrictions. The SQLite
snapshot can retain absolute paths and source-derived properties; compressing
it does not remove those contents. Keep raw databases and private diagnostics
out of public benchmark reports. Downloaded manifests are checked against the
producer's independently recorded immutable identity, not trusted merely
because they accompany the archive.

Regenerate after each accepted source change that needs a shared generation.
Use a bounded retention policy for old source/build keys. Test consumer download
and startup on representative runners, at different checkout paths, and
repeat the same readiness and resource measurements before expanding use.
An older artifact may be an explicitly measured incremental input; it cannot
stand in for current source evidence before those checks pass.

Treat a provider upgrade as a new producer key. Pin and audit the executable,
regenerate from clean source, and repeat the matrix and compatibility checks.
The malformed-schema observation in this experiment proves rejection/recovery
for one controlled input; it does not test a future provider build.

On missing, corrupt, incompatible or incorrectly bound data, preserve the
refusal, quarantine the downloaded artifact and use a clean indexing path.
Include failed work in latency and resource accounting. Disable artifact
download to roll back a pilot; source remains authoritative. Rebuilding an
existing production cache still follows Temper's
[maintenance safeguards](../../how-to/recover-codebase-memory.md).
Benchmark ownership of disposable state does not authorize operator-cache
deletion or removal of maintenance identity checks.

This work installs no shared producer, changes no default index arguments and
does not make a bounded pilot candidate equivalent to broad adoption.
