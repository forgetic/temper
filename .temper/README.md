# Temper automation

Run the pre-PR validation script from the repository root before pushing or
opening an implementation PR:

```sh
./.temper/pre-pr
```

The script runs these checks in order and stops on the first failure:

1. Rust formatting
2. Dependency-graph policy
3. Rust file-size policy
4. Ambient-environment access policy
5. Cached custom-harness permission repair against mode-0644 fixtures
6. Workspace test prebuild
7. Quick nextest build, custom-harness repair, and test execution
8. Linked test-binary cleanup
9. Clippy

The quick nextest step captures a binaries-only build before repairing
`benchmark_harness`, `linux_supervisor`, and `windows_job`. Nextest then reuses
the captured Cargo and binary metadata, so no cache restoration can replace the
repaired artifacts between the final build and test enumeration.

`.temper/pre-push.toml` wires the same script into Temper's `submit_for_pr`
pre-push gate for writable engineer workspaces.
