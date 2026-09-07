# Upgrade and verify codebase-memory-mcp

The tested deployment release is
[`v0.10.8`](https://github.com/DeusData/codebase-memory-mcp/releases/tag/v0.10.8).
Temper launches an external executable; Cargo does not install or pin it.
Check both `command -v codebase-memory-mcp` in the test environment and the
resolved `[agent.tools.codebase_memory] command` in each worker deployment.
Run each selected executable with `--version`; the paths may point to the same
installation.

Set `startup_timeout_secs = 30` in the deployment's codebase-memory settings.
The 0.10.8 cold daemon startup exceeded the former five-second setting on the
development/production host; checking `--version` alone does not exercise it.
Provider `args` are passed verbatim, including repeated or empty values. Do not
deduplicate them: repeated positional values are required by test launchers.

## Stage and check the release

Download the appropriate archive and `checksums.txt` from that exact release,
verify the archive checksum, and extract it into a staging directory. Run the
staged executable with `--version` and confirm `codebase-memory-mcp 0.10.8`.
Use the portable Linux archive on hosts without the standard build's required
glibc version.

From the Temper repository, put the staged directory first on `PATH` and run:

```sh
PATH=/absolute/staging/directory:$PATH cargo dev-test-build
PATH=/absolute/staging/directory:$PATH cargo nextest run --workspace \
  -E 'test(installed_provider_release_supports_temper_graph_tools)' --run-ignored only
```

The build includes the shared-owner helper used by the ignored Linux smoke test.
The test requires the actual 0.10.8 executable. It checks MCP
initialization/version and Temper's provider contract, indexes a tiny repository
through Temper's blocking stable-project binding, then exercises graph search,
source snippets, caller tracing, code search, and cited-path/scoped coverage through
the agent tool wrappers. A controlled `.cbmignore` exclusion must be flagged and
a changed source file must produce stale coverage.
It sets both `CBM_RUNTIME_DIR` and `CBM_CACHE_DIR` to private temporary directories
so its daemon and indexes use private state. On a shared interactive host, run
the already-built ignored test executable under a dedicated test account/UID as
an additional isolation boundary; do not stop operator sessions or delete caches.

The quick suite and scenario fixtures use fake MCP servers advertising 0.10.8.
Temper rejects older releases. The fixtures' advertised version
is not evidence that a particular upstream release works. The real-provider
smoke is deliberately ignored in the quick suite and must be run explicitly for
provider upgrades. It is provider compatibility coverage, not mapped live
scenario or agent-effectiveness evidence.

## Activate and verify the deployment

Deploy a Temper build containing the released-provider response handling before
activating 0.10.8. Temper requests JSON for graph tools and expands the release's
columnar tables into explicit records for typed source-selection evidence.
Older builds can also skip initial indexing because the missing-project response
has an `error` instead of a `status`.

Wait for active worker jobs to finish and quiesce processes that can start a
provider. Keep the previous executable for rollback. Use the verified staged
binary's native installer to activate it at the deployment's configured path:

```sh
/absolute/staging/directory/codebase-memory-mcp install -y --force \
  --skip-config --dir=/absolute/installation/directory
```

`--force` replaces an existing binary; `--skip-config` preserves coding-client
configuration. The native installer coordinates active provider shutdown before
activation and preserves existing indexes. Restart worker services if stopped,
and reconnect open coding-client MCP sessions so they launch the new executable.
Do not delete the provider cache to upgrade it.

The native installer rejects group-writable or world-writable staging and
installation directories. Use owner-controlled directories with mode `0755`
or stricter; a directory created with mode `0775` must be corrected first.

Verify the installed executable's version and repeat the smoke test with its
directory first on `PATH`. Check worker health and the first provider startup.
Temper rebinds/indexes the prepared checkout when a job starts, so subsequent
indexing uses the upgraded provider. If an explicit production cache rebuild is
needed, follow [Recover a codebase-memory cache safely](recover-codebase-memory.md).

The 0.10.8 inventory uses `offset`/`limit` pagination and does not expose the
cache instance identity required by Temper's deletion safeguards. Maintenance
therefore fails closed without deleting projects; the graph-tool smoke does not
claim to validate cache reclamation. Do not bypass that identity requirement.

## Keep one account cohort consistent

Use the same executable build, native coordination ABI and canonical cache root
for every active provider session under an OS account. Version text alone does
not establish build identity: record the installed executable digest alongside
its release and platform archive. Resolve symlinks in cache paths before comparing
settings. `CBM_RUNTIME_DIR` changes storage paths; it is not an account-isolation
boundary. Per-job cache overrides under one shared account are test fixtures,
not a production isolation strategy.

Keep daemon-global environment settings consistent, including cache/runtime
placement, worker limits, tracing and memory settings. The first activating
session establishes the daemon configuration. Temper passes operator-resolved
worker invocation overrides to its bootstrap as well as to its serving child.
Session-specific allowed-root and tool-profile restrictions have a different
scope; a bootstrap analysis profile does not narrow the serving session's tools.

To change the build, ABI, cache root or daemon-global settings, first stop new
admissions and finish/stop every affected account session, including interactive
clients and other workers. Preserve existing indexes and use the native staged
installer above. Reconnect affected sessions only after activation completes.
Do not replace the executable beneath active sessions, delete coordination files,
or repeatedly spawn through a reported version/build/cache-root conflict.

Native admission and activation are bounded operations that can report cohort
conflicts. Inspect the provider's daemon, conflict and activation diagnostics in
the configured runtime/cache locations. In this build the canonical cache's
`logs/cbm-daemon.log` records daemon operations and `logs/daemon-conflicts.ndjson`
records cohort conflicts. Retain the closed failure category,
release/digest and operation timing rather than copying source, secret-bearing
arguments or host paths into durable Temper telemetry. A one-shot CLI request
participates in native admission and mutation coordination, then releases its
session; it is not a permanent keepalive. `daemon start` explicitly requests a
permanent daemon and is not Temper's lifecycle workaround.

Temper's Linux worker/standalone shared owner preserves other sessions through
individual job cancellation. Whole-worker/service shutdown still requires orderly
handling of affected account sessions; see
[Shared codebase-memory lifetime](../explanation/shared-codebase-memory-lifecycle.md).
