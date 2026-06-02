# Phase 6 — Dogfood/reference migration and re-enable criteria

You are implementing Phase 6 of `plans/user-defined-role-agents/README.md`.
Phases 1–5 should be complete: production role agents are manifest-driven,
external tools are user-declared/bound, and a safe coding workspace seam exists.

The goal is to migrate dogfood/reference configuration to the new model and make
engineer automation safe to enable only when the coding workspace prerequisite is
satisfied.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `plans/user-defined-role-agents/README.md`, and Phases 1–5
   diffs.
3. Read dogfood/reference docs and scripts:
   - `examples/dogfood/README.md`
   - `examples/dogfood/run.sh`
   - `examples/dogfood/config/dogfood.env`
   - `examples/dogfood/tools/parse_secrets.py`
   - `examples/dogfood/tools/configure_forgejo.py`
   - `docs/how-to/run-cross-repo-reference-delivery-demo.md`
   - `examples/reference-delivery/README.md`
4. Read production worker/provisioning code and the coding workspace binding from
   Phase 5.

## Task

Move reference/dogfood role behavior into user configuration and add clear
preflight/re-enable rules.

1. **Reference workflow guidance.** Put any remaining reference-delivery role
   guidance needed for demos/tests into workflow config or test/demo fixtures,
   not production `temper-agents` prompt files. Keep generated mechanics
   separate from user guidance.

2. **Dogfood config.** Update dogfood configuration so engineer automation can be
   enabled only when:
   - the workflow role declares the coding workspace external tool;
   - the runner has a coding workspace binding;
   - PR diff guard settings are active;
   - required credentials/workspace paths are present.

   Keep the default disabled unless the repo owner explicitly chooses otherwise.

3. **Preflight/explain idle issues.** Add or update a dogfood preflight/check that
   explains why an eligible `code + ready` issue is idle when the engineer role
   lacks a safe coding binding. The message should point at the specific config
   keys and should not suggest enabling synthetic PR prep.

4. **End-to-end dogfood proof.** Use a real dogfood issue, or a throwaway local
   Forgejo e2e fixture if dogfood credentials are unavailable, to prove the
   workspace path can produce a meaningful PR diff and pass the guard. Do not
   create bookkeeping-only PRs.

5. **Docs.** Update dogfood and reference-delivery docs with:
   - how generated prompts/user prompt extensions work;
   - how to declare/bind the coding workspace tool;
   - why engineer automation may be idle;
   - exact commands to validate the setup.

6. **Tests.** Add focused shell/Python/Rust tests for new preflight/config logic,
   plus e2e coverage through the existing real-agent suites.

7. **Plan status.** Mark Phase 6 complete only after the full closure suite
   passes and the re-enable criteria are documented.

## Constraints

- Do not casually flip `DOGFOOD_ENABLE_ENGINEER_AUTOMATION` to enabled. If you
  enable it for a live validation, record the issue/PR and prove the diff is real.
- No synthetic/bookkeeping-only PRs in live dogfood.
- Secrets must stay in env or `0600` secret files, never argv/logs.
- Do not add role behavior back into production prompt files.

## Done

Run and record the full closure suite from the plan README:

```sh
cargo fmt --all
cargo test --workspace --all-targets
cargo dev-clippy
cargo dev-check
cargo test -p temper-testing --test multiprocess -- --ignored --test-threads=1
cargo test -p temper-testing --test multi_repo_multiprocess -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-agents --test forgejo_engineer_e2e -- --ignored --test-threads=1
```

Also run touched Python/shell tests, configured live provider gates, and any live
dogfood validation that was used. Follow `docs/how-to/end-a-development-session.md`
and update the phase status.
