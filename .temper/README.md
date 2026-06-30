# Temper automation

Run the pre-PR validation script from the repository root before pushing or
opening an implementation PR:

```sh
./.temper/pre-pr
```

The script runs these commands in order and stops on the first failure:

1. `cargo dev-fmt`
2. `cargo dev-scenario-check`
3. `cargo-clippy`
4. `cargo dev-test-quick`
5. `cargo dev-test-e2e-all`

`.temper/pre-push.toml` wires the same script into Temper's `submit_for_pr`
pre-push gate for writable engineer workspaces.
