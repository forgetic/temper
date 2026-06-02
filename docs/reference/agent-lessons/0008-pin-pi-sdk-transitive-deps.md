# Lesson 0008: Pin `pi_agent_rust`'s transitive deps when the SDK won't compile

## Tags

`tooling`, `rust`, `agents`, `dependencies`, `pi-sdk`

## Trigger

Scaffolding `crates/harness-agents` (Phase B1) added `pi_agent_rust = "0.1.13"`.
The very first `cargo check` failed — not in our code, but deep inside a
transitive dependency, `asupersync 0.3.1`, with errors like `no field
'fallback_active' on type 'Result<DecisionOutcome, …>'`.

## What went wrong

`pi_agent_rust 0.1.13` pins `asupersync =0.3.1`, which declares
`franken-decision = "0.3.1"` (a **caret** range). A fresh resolve picked
`franken-decision 0.3.2`, whose `evaluate()` had changed to return a `Result`,
while `asupersync 0.3.1`'s code still uses the bare `DecisionOutcome`. The SDK
therefore does not build with a default resolve, even though the installed
toolchain (1.93.1) is new enough. `pi_agent_rust`'s own committed `Cargo.lock`
had pinned `franken-decision 0.3.1`, which is API-compatible — the breakage only
appears in a downstream workspace that re-resolves.

The assumption "the toolchain builds the SDK, so adding the dependency just
works" was wrong: a transitive caret dependency had drifted to an incompatible
patch.

## Steering for future agents

- When a freshly added crate fails to compile **inside a transitive
  dependency**, suspect version skew before touching the dep's source. Run
  `cargo tree -i <crate>` to find who pins it, and compare the resolved version
  against the upstream crate's own `Cargo.lock`
  (`~/.cargo/registry/src/*/<crate>-<ver>/Cargo.lock`).
- Fix by pinning the drifted dependency to the upstream-tested version in **our**
  `Cargo.lock`, not by editing vendored source or relaxing constraints:
  `cargo update -p franken-decision --precise 0.3.1`. Keep that pin; a future
  `pi_agent_rust` bump may change the constraint, so re-check then.

## Where this is now documented

- `plans/forgejo-e2e/findings-phase-b.md` (the "CRITICAL gotcha" section).
- `docs/reference/llm-agents.md` (the dependency note for `harness-agents`).
- The pin itself lives in the workspace `Cargo.lock`.
