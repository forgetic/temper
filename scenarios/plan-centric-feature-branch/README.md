# Plan-centric feature branch scenario

This live scenario validates the #144 dogfood workflow shape on real Forgejo,
real `forgejo-runner`, a real Temper process, and Jig fake LLM responses.

It exercises:

1. a `feature` issue;
2. architect creation of a `plan` issue carrying feature-branch metadata;
3. dependency-linked `code` children where the second child is blocked on the
   first;
4. lineage delivery at every agent boundary: the versioned artifact bundle,
   primary legacy work-item content, ancestry, and plan-validation summaries;
5. implementation PRs landing into the feature branch;
6. tester validation creating an aggregate `feature_landing_pr` from the feature
   branch to `main`;
7. one validated audit record visible through the coordinating plan's ordinary
   Forgejo comments API, including the safe tester summary, role/actor identities,
   and job/transition/coordination identifiers;
8. final landing closing both plan and feature issues.

Run with:

```sh
cargo run -p temper-scenario-cli -- run --tier live --temper-bin target/debug/temper scenarios/plan-centric-feature-branch
```
