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
5. Workspace test prebuild
6. Test executable integrity regressions and native-header checks
7. Quick nextest test execution
8. Linked test-binary cleanup
9. Clippy

The integrity guard uses nextest's binaries-only inventory without executing
test harnesses. Missing or damaged executable headers fail validation before a
corrupt retained output can silently appear as an empty test suite. It reports
the affected paths and leaves the outputs available for inspection and rebuild.

The repository-local kache configuration excludes the three `harness = false`
test targets that kache 0.11 cannot recognize as extensionless executables.
The ordinary Cargo and nextest commands therefore need no permission repair.

`.temper/pre-push.toml` wires the same script into Temper's `submit_for_pr`
pre-push gate for writable engineer workspaces.
