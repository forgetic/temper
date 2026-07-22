# Plan-centric feature branch scenario

This validation-grade live scenario reproduces feature #620's production failure
shape on real Forgejo, real `forgejo-runner`, a real Temper process, and captured
Jig fake-LLM requests. The source feature intentionally arrives with
`target_branch: main`; that metadata is treated as untrusted input rather than
feature-delivery intent.

It proves:

1. the architect omits the engine-derivable branch, and Temper stamps
   `agent/pr-for-feature-<feature-number>` dynamically;
2. the derived branch differs from `main` and appears on the plan, both initial
   code children, and a tester-requested validation follow-up;
3. every implementation PR targets the derived branch, each successful merge
   advances that branch in sequence, and `main` remains at its initial SHA;
4. initial validation waits for both decomposed implementations, requests a
   follow-up, and final validation waits for that follow-up implementation;
5. exactly one aggregate feature-landing PR is created from the feature branch
   to `main`;
6. successful CI precedes every merge, including the aggregate landing;
7. the plan and feature remain open while the landing PR is open and close only
   after its merge advances `main`;
8. every captured architect, engineer, and tester prompt contains its configured
   role charter/prompt guidance, external-tool guidance, and tool constraints;
9. both tester rounds leave assignment-bound ordinary-comment audit evidence.

Intentional default-branch delivery is **not** modeled by this scenario. Its
explicit `repository_default` same-branch convergence remains a distinct engine
regression (`verdict_transition_treats_default_branch_source_as_satisfied_create`),
so accidental `main` metadata here cannot be confused with declared intent.

Run with the live command required by `AGENTS.md`:

```sh
cargo dev-scenario-run scenarios/plan-centric-feature-branch
```

Equivalent direct command after building `temper`:

```sh
cargo run -p temper-scenario-cli -- run --tier live --temper-bin target/debug/temper scenarios/plan-centric-feature-branch
```
