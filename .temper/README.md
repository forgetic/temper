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
6. Quick nextest test execution
7. Linked test-binary cleanup
8. Clippy

The repository-local kache configuration excludes the three `harness = false`
test targets that kache 0.11 cannot recognize as extensionless executables.
The ordinary Cargo and nextest commands therefore need no permission repair.

`.temper/pre-push.toml` wires the same script into Temper's `submit_for_pr`
pre-push gate for writable engineer workspaces.
