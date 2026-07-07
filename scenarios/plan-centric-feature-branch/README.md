# Plan-centric feature branch scenario

This live scenario validates the #144 dogfood workflow shape on real Forgejo,
real `forgejo-runner`, a real Temper process, and Jig fake LLM responses.

It exercises:

1. a `feature` issue;
2. architect creation of a `plan` issue carrying feature-branch metadata;
3. dependency-linked `code` children where the second child is blocked on the
   first;
4. implementation PRs landing into the feature branch;
5. tester validation creating an aggregate `feature_landing_pr` from the feature
   branch to `main`;
6. final landing closing both plan and feature issues.

Run with:

```sh
cargo run -p temper-scenario-cli -- run --tier live --temper-bin target/debug/temper scenarios/plan-centric-feature-branch
```
