# Graph artifact benchmark

This opt-in Linux benchmark compares the audited native codebase-memory-mcp
0.10.8 build across independent Temper checkouts. It does not change Temper's
default persistence setting. The checked-in
[experiment](../../docs/explanation/codebase-memory-artifacts/experiment.md)
and [configuration](../../docs/explanation/codebase-memory-artifacts/config.json)
define the measured dataset and decision rule. The [measured decision](../../docs/explanation/codebase-memory-artifacts/decision.md)
defers shared artifacts and links every native observation.

## Run the synthetic workflow

Python 3.11 or later and `zstd` must be on PATH. No Python packages are required.
From the repository root:

```sh
python3 -m unittest discover -s benchmarks/codebase-memory-artifacts/tests -v
python3 benchmarks/codebase-memory-artifacts/benchmark.py fixture --output /var/tmp/artifact-fixture.json
```

The fixture exercises this harness's real manifest validation, zstd/SQLite
inspection, root/source/symbol/call/coverage checks, quarantine, recovery and
report projection. Its provider adapter is explicitly synthetic. Its 25 sample
rows and four failure rows contain no native timing values and cannot qualify
for an adoption recommendation. Synthetic SQLite stores a fixed synthetic root
so repeated complete reports are deterministic; source gates still check the
actual temporary checkout paths and bytes. The mapped Jig scenario copies these
same nine Python modules and runs this command through real delivery and host CI.

## Prepare and freeze a native run

The native executable must have SHA-256
`cb6dc545a8cb714799461ba7d8c223dbca7d02ca46ea5a4215c1beb450fd8e09`.
The runtime verifies this before starting it. Use `config.example.json` to
prepare another experiment. Pin full P0/P1 Git commit IDs; P1 must descend from
P0 and include add, edit, delete and rename operations. Both commits must be
available in the source repository. List exact symbol names, relative files,
line ranges and SHA-256 values of the range bytes including newlines, both
ends of every expected call, coverage paths, and P1 negative symbols/files.

```sh
python3 benchmarks/codebase-memory-artifacts/benchmark.py inspect-source --repository . --commit FULL_P0_COMMIT
python3 benchmarks/codebase-memory-artifacts/benchmark.py validate-config --config docs/explanation/codebase-memory-artifacts/config.json
```

The inspection command reports the source identity and controlled clone ignore
identity without starting the native provider. The configuration must specify
at least five repetitions, an exact permutation of all five cells for each
repetition, request/shutdown deadlines, account ID, sampling interval, mode and
the adoption threshold. Native worker environment defaults remain observable
defaults unless explicitly pinned through the audited `CBM_INDEX_SINGLE_THREAD`
or `CBM_PROFILE` options. External Git configuration and templates are disabled;
tracked nested `.gitignore`, `.cbmignore` and `.codebase-memory.json` files are
hashed, and clones use a read-back empty info/exclude and external exclude file.

Save the exact configuration and harness revision before comparing cells.
`validate-config` emits the exact configuration-byte checksum. Then run from
the repository root, using a fresh task directory on disk and a dedicated
unused UID/GID that differs from both root and the invoking operator:

```sh
sudo -n python3 benchmarks/codebase-memory-artifacts/benchmark.py run \
  --config docs/explanation/codebase-memory-artifacts/config.json \
  --frozen-sha256 FROZEN_CONFIG_SHA256 \
  --state-root /var/tmp/temper-artifact-run-UNIQUE \
  --output /var/tmp/artifact-native-report.json
```

The portable provider command is resolved through PATH before privilege/session
operations. If `sudo` uses a restricted PATH, preserve an explicitly reviewed
PATH for the command or freeze a configuration with the executable's absolute
path. Source and executable paths are runtime locations, omitted from reports.
The harness refuses existing state and output paths. It prints only closed
sample progress on stderr. Its private evidence includes the runtime-resolved
configuration, host details, every MCP request/response, initial failure and
recovery outcomes, process identities, captured worker startup headers, cache
contents and all sample rows. Do not publish that directory.

## Lifetime, measurements and failures

The root Python supervisor is a Linux child subreaper. Public MCP frontends run
under the dedicated account, at the explicit checkout CWD. Public `config set`
and `config get` disable automatic indexing, watching and UI in the task's
canonical cache before the first session. An analysis-profile bootstrap keeps
account admission continuous; mutation uses the default ALL profile (there is
no `--tool-profile=all` flag). Queries use fresh analysis readers, avoiding
cached old generations. No daemon-start or private provider ABI is used.

Every frontend closes normally; the supervisor waits for native account exit
and reaps adopted descendants. Timeout TERM/KILL is emergency cleanup and
invalidates natural-lifetime evidence, including when a client fails during
initialization or warm preparation. Cache rotation happens only after the
account is empty. The canonical configuration stays in place while task-owned
database/log state moves into private evidence. Checkouts use `--no-hardlinks`
before ownership changes; symlink ownership changes never follow targets.

RSS samples sum all dedicated-account processes, including daemon and indexing
workers. CPU uses inclusive child usage after native exit, which also includes
control subprocesses run inside that session; it is job subprocess CPU, not
frontend-only or separately attributed provider CPU. Warm P0 preparation is
separate; settled live daemon CPU is subtracted at the P1 boundary with Linux
clock-tick precision. Sampling stops and restarts across that boundary.
Worker startup headers are captured while present because successful native
worker logs can disappear. Missing headers never prove that no work occurred.

Ready-and-correct time begins after checkout and before transfer/startup; job
time also includes checkout and natural shutdown. Warm P0 preparation and the
single producer/configuration setup are separately reported. The report retains
every attempted observation and emits no success-only median for an incomplete
cell. It reports median/min/max for complete cells. A pilot candidate requires
the frozen improvement in both matched ready-and-correct comparisons, no job
regression, and every initial correctness/lifetime gate. Local copy measures
local transport only; producer fanout and remote-download costs need separate
interpretation. Native import, export/compression and incremental costs lack
independent public timers, so combined request durations are retained. A
validated artifact manifest is not proof that native import occurred; observed
index status and artifact presence are recorded separately.

Missing, corrupt, truncated and incompatible-schema artifacts first pass
through the same immutable-manifest gate as normal consumers. Disposable raw
provider probes then measure what the installed provider does with those exact
bad inputs. This deliberately bypasses the wrapper only inside the isolated
experiment. The post-probe artifact directory is quarantined and recovery uses
a fresh named project with no artifact; it never deletes or repurposes operator caches. Foreign-root
or stale-source outcomes remain initial refusals even if recovery succeeds.
Native indexing can refresh an existing artifact even with persistence disabled;
quarantine therefore need not retain the original malformed bytes. The measured
[decision](../../docs/explanation/codebase-memory-artifacts/decision.md) records
this behavior and its evidence limits. Schema mutation tests rejection; it
cannot establish next-version compatibility.

The manifest binds source, provider version/build, configuration/ignore
identity, artifact/metadata checksums, byte sizes and actual SQLite inspection.
The SQLite data contains source/path information; distribute it only through
repository-appropriate access. Published projection omits raw source, runtime
paths, process commands and diagnostic text. Review sanitized output before
publication. All retained task state can later be removed, after saving the
needed private evidence, with the ownership-checked command:

```sh
sudo -n python3 benchmarks/codebase-memory-artifacts/benchmark.py cleanup --state-root /var/tmp/temper-artifact-run-UNIQUE
```

Cleanup requires the original marker/device/inode/owner and an empty dedicated
account. There is no generic cache deletion or persistence-enable command.
