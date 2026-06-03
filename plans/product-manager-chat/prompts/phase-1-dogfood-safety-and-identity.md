# Phase 1 — Dogfood safety rails + product-manager identity

Implement the dogfood-side prerequisites for product-manager conversations. This
phase should not add the chat agent yet; it makes the live repo safe for product
transcript issues and makes a separate `product-manager` Forgejo identity
available to later phases.

## Bootstrap

1. Follow the normal session bootstrap in `AGENTS.md`.
2. Read:
   - `plans/product-manager-chat/README.md`
   - `examples/dogfood/README.md`
   - `examples/dogfood/run.sh`
   - `examples/dogfood/tools/label_intake.py`
   - `examples/dogfood/tools/parse_secrets.py`
   - `examples/dogfood/tools/configure_forgejo.py`
   - `examples/reference-delivery/run.sh` for the current long-running launcher
     snapshot/teardown pattern
3. Keep this repo focused on workflow/integration surfaces. Do not add a web UI.

## Goals

- A Forgejo issue labeled `product` must **not** be relabeled `untriaged` by the
  dogfood intake labeler.
- The live dogfood setup can parse and grant access to a non-workflow
  `product-manager` user/token when the private credential note contains one.
- The `product` label is ensured by dogfood setup, even though it is not a
  workflow label.

## Implementation details

### Intake labeler safety

Update `examples/dogfood/tools/label_intake.py` so issues carrying `product` are
considered intentionally classified/non-intake and skipped.

Today the labeler skips only workflow identifying labels:

```python
WORKFLOW_IDENTIFYING_LABELS = {"untriaged", "epic", "design", "code", "implementation"}
```

Add a separate non-workflow skip set, for example:

```python
NON_WORKFLOW_SKIP_LABELS = {"product"}
```

The labeler should skip when an issue has either a workflow identifying label or
a non-workflow skip label. Keep the behavior for unlabeled issues unchanged:
new unlabeled issues created after the dogfood run starts still get `untriaged`.

### Product label setup

Extend `examples/dogfood/tools/configure_forgejo.py` to idempotently ensure a
repo label named `product` with a clear description such as:

> Product discussion and planning records that are not workflow intake.

Use the Forgejo REST API directly, matching the style of the existing script:
list labels, create if absent, patch if present and missing description/color.
Do not print tokens.

### Product-manager identity

Extend `examples/dogfood/tools/parse_secrets.py` to support a
`product-manager` credential from the private note. Requirements:

- Add a config/default in `examples/dogfood/config/dogfood.env`:

  ```sh
  DOGFOOD_PRODUCT_MANAGER_USER=product-manager
  ```

- Accept a `--product-manager-user` flag in `parse_secrets.py`, defaulting to
  `product-manager`.
- Complete an alias named `product-manager` from that configured source user,
  analogous to how `human` can alias `bot`.
- When a product-manager token is present, emit:

  ```sh
  TEMPER_FORGEJO_USER_PRODUCT_MANAGER=...
  TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER=...
  TEMPER_FORGEJO_PASSWORD_PRODUCT_MANAGER=...
  ```

- Include the product-manager user in `DOGFOOD_PERMISSION_USERS` when present,
  so `configure_forgejo.py` grants repo write access.
- Do **not** make product-manager a required workflow role. Missing credentials
  should not break normal `./run.sh start`; later `product-chat` should fail
  clearly if the token is missing.

Update `examples/dogfood/run.sh` so `parse_live_secrets` passes the configured
`--product-manager-user` value.

## Tests / validation

Add lightweight tests for the Python helpers without relying on the live Forgejo
server. A pytest dependency is not required; prefer standard-library `unittest`
or a small test script that can be run with Python.

Minimum coverage:

- `label_intake.py` would label a new unlabeled issue.
- `label_intake.py` skips an issue labeled `product`.
- `parse_secrets.py` emits product-manager env vars and includes the user in
  `DOGFOOD_PERMISSION_USERS` when the source note has credentials.
- `parse_secrets.py` still succeeds when product-manager credentials are absent.

Run:

```sh
python3 -m py_compile examples/dogfood/tools/*.py
python3 -m unittest discover examples/dogfood/tools
cargo fmt --all
cargo dev-check
```

If adding tests under a different path, document the exact command in this
README phase status when marking it done.

## Documentation updates

- Update `examples/dogfood/README.md` to mention:
  - `product` issues are not workflow intake;
  - `product-manager` credentials are optional for normal dogfood workers and
    required only for product-chat once Phase 3 lands.
- Update `plans/product-manager-chat/README.md` phase status when complete.

## Acceptance criteria

- Product transcript issues are not auto-labeled `untriaged` by the dogfood
  labeler.
- The dogfood setup ensures the `product` label.
- Product-manager credentials can be parsed and granted repo access without
  affecting normal role workers.
- No web UI is added.
