# Phase 6 prompt — Documentation, knobs, and performance acceptance

## Goal

Close the plan by documenting the scalable scan contract, production knobs, and
observed speedups. Add tests or smoke checks that protect against regression to
broad scans and eager CI web-UI reads.

## Required reading

- Completed Phases 1–5
- `docs/reference/workflow-layer.md`
- `docs/reference/forge-interface.md`
- `docs/reference/forgejo-backend.md`
- `docs/how-to/run-forgejo-multiprocess-e2e.md`
- `docs/explanation/forgejo-e2e-topology.md`
- `docs/how-to/fast-local-iteration.md`
- `docs/how-to/end-a-development-session.md`

## Implementation tasks

1. Update reference docs with the final scan behavior:
   - role-targeted queue scans;
   - lazy gate-signal reads;
   - closed-history pruning;
   - audit/poll backstop semantics;
   - hinted repo narrowing.
2. Document any production CLI/config knobs added for audit interval, scan mode,
   or diagnostics.
3. Update Forgejo e2e how-to with expected timing shape and troubleshooting for
   scan counts / CI web-UI reads.
4. Add regression tests that are cheap enough for the default suite:
   - many closed unlabelled artifacts do not affect scan result/count;
   - non-CI role scans do not call CI;
   - hint for repo A does not scan repo B on immediate wake.
5. Add optional ignored/live diagnostics for Forgejo that print per-scenario
   timing, scan counts, and CI web-UI reads.
6. Record before/after timings in this plan README or a small `findings.md` in
   this plan directory.
7. If any durable lesson emerged, add an agent lesson and/or promote it into the
   canonical docs.

## Validation

Run the normal closeout unless the final implementation notes justify a narrower
set:

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
cargo dev-test
```

Then run the self-contained ignored Forgejo tests if the binary cache is present:

```sh
cargo test -p temper-testing -- --ignored --test-threads=1
```

## Done when

- Docs explain why normal ticks do not scale with closed history.
- Regression tests guard the key performance properties.
- Ignored Forgejo timing expectations are documented.
- The plan README marks all phases complete and links to timing/findings notes.
