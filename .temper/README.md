# Temper automation

Run the pre-PR validation script from the repository root before pushing or
opening an implementation PR:

```sh
./.temper/pre-pr
```

The script runs these commands in order and stops on the first failure:

1. `cargo dev-fmt`
2. `cargo-clippy`
3. `cargo dev-test-quick`
4. `cargo dev-test-e2e-all`

`.temper/pre-push.toml` wires the same script into Temper's `submit_for_pr`
pre-push gate for writable engineer workspaces.
