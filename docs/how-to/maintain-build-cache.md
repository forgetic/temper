# Maintain the build cache

Codex builds, Temper engineer workspaces, and Forgejo's host runner share the
`runner` account's kache store. Run `.temper/pre-pr` before submitting either
kind of PR. Temper's `.temper/pre-push.toml` runs that script at submission.
CI builds the same `cargo dev-test-build` graph in its persistent workspace.

## Host configuration

Use kache **0.26.3 or newer** for both Cargo's `rustc-wrapper` and the systemd
daemon. Keep `~/.local/bin/kache` pointing to the packaged `/usr/bin/kache` so
PATH lookup, Cargo's absolute wrapper path, and the service cannot drift apart.
After upgrading, restart `kache.service` and check both versions:

```sh
kache --version
kache daemon status --json
systemctl show kache -p ExecStart
```

The daemon's version must match the wrapper. Its system service is not the
user service that kache's `service_installed` field checks.

Both `.temper/pre-pr` and CI run `scripts/prepare-build-cache.py`. It records
the kache version in Cargo's configured target directory and runs `cargo clean`
once if the version changes or the marker is missing. Cargo does not otherwise
notice a replaced wrapper binary; retaining old dependencies can mix different
path-remapping formats and prevent reuse even after upgrading kache. Subsequent
runs retain the target. This does not clear the shared kache store.

Put machine-wide cache capacity in **`/etc/kache/config.toml`**. For this host:

```toml
[cache]
local_store = "/srv/data/git/runner/.cache/kache"
local_max_size = "235GB"
cache_executables = true
```

A project's `.kache.toml` replaces `~/.config/kache/config.toml`; it does not
merge that user file. It *does* inherit `/etc/kache/config.toml`. Keeping the
capacity only in the user file gives the daemon and compiler wrappers different
eviction budgets. The store limit is not a quota for Cargo target directories;
monitor filesystem free space separately.

Leave `build.jobs` unset in both the repository and `~/.cargo/config.toml`.
The old host-wide `jobs = 1` serialized every miss, including test linking.
Use the same toolchain, profile, rustflags, and Cargo target scope locally and
in CI. A narrow build or `cargo check` alone does not warm the complete test
graph. Never normalize away environment values that a binary uses as real
runtime paths, such as `CARGO_MANIFEST_DIR` or `CARGO_BIN_EXE_*`.

Scenario fixture inheritance searches the manifest's workspace and the current
working directory's workspace. It never searches the checkout used to compile
Temper. Run an external bundle from its intended workspace, or keep inherited
fixtures beside the bundle. This makes the scenario library relocatable and
keeps its dependents' compilation keys stable across agent and CI checkouts.

## Validate an upgrade

```sh
python3 -B -m unittest discover -s scripts/tests -p test_build_cache.py -v
./.temper/pre-pr
cargo dev-scenario-check
```

The cache regression uses an isolated store. It creates separate keys sharing
output blobs, runs GC, restores into a new checkout, and executes a restored
`harness = false` binary. It skips when Cargo or kache is absent; CI explicitly
requires kache. The pre-PR lane also checks real workspace executable headers
and runs the quick suite.

For full-workspace performance validation, build the same commit in separate
checkouts with separate targets and one shared store. Measure the first build
and the subsequent checkout builds. Rebuilding an already-fresh target only
measures Cargo's fingerprint check. Run real tests after restoration too.

## Diagnose a slow PR

The workflow prints the cache version/configuration before Build and a report
restricted to that build's time window and source root afterward. Reports also
record local session history in the store. For local investigation:

```sh
kache report --root "$PWD" --since 1h
kache why-miss temper_engine
```

`Compiling` in Cargo's output does not distinguish a cache hit from rustc work.
Use kache's hit, miss, duplicate, compile-time, and restore-time counters.
`dup` means compilation ran and produced blobs already stored; it is not a hit.
The repository enables `explain_miss` to preserve key-component diagnostics.

The September 2026 investigation found an 11m27s PR build with 94 hits,
216 duplicate compiles, and four new misses. All 216 duplicate keys had
`duplicate` eviction tombstones. They consumed 640 seconds of wrapper time,
including 584 seconds of compiler work. The old GC removed useful keys despite
their blobs remaining shared. The upgraded GC retains keys when their removal
would reclaim no storage. Also fixed: the 0.12 wrapper / 0.11 daemon split,
the hidden user-level capacity setting, and the one-job Cargo limit. A second
cross-checkout rebuild cascade started at the scenario library's embedded
`CARGO_MANIFEST_DIR`; removing that fallback fixes the cause instead of ignoring
a meaningful path in the cache key.

See the [upstream GC contract][gc] and [configuration precedence][config].

[gc]: https://github.com/kunobi-ninja/kache/blob/v0.26.3/crates/kache-store/src/eviction.rs
[config]: https://kunobi.com/docs/kache/getting-started/configuration
