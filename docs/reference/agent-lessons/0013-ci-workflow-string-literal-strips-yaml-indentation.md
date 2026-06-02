# Lesson 0013: Don't build indented YAML with `\`-continued string literals

## Tags

`forgejo`, `ci`, `rust`, `provisioning`, `temper-production`

## Trigger

Running `examples/reference-delivery/` end-to-end: the LLM agents triaged,
opened a PR, and approved it, but it never merged. CI never ran — `action_run`
was empty and `GET /repos/.../actions/workflows` returned no workflows, even
though `.forgejo/workflows/ci.yml` existed in the repo tree.

## What went wrong

`temper-production`'s `CI_WORKFLOW` constant was a normal string literal using
`\<newline>` line continuations to wrap the source:

```rust
"jobs:\n\
  build:\n\          // the two leading spaces here are STRIPPED
    runs-on: host\n\
```

Rust's `\<newline>` continuation removes the newline **and all leading
whitespace** of the next source line. The author used that leading whitespace as
the YAML indentation, so the committed workflow landed completely flush-left:

```yaml
jobs:
build:
runs-on: host
```

That is invalid workflow YAML. Forgejo **silently** fails to detect it (no error
logged at `LEVEL = error`), so no run is ever scheduled on push and the whole
pipeline stalls with a green-looking PR that can't pass its CI gate. The working
`temper-testing` copy avoided this by encoding every indent space as a `\u{20}`
escape (which the continuation does not strip).

## Steering for future agents

- Never rely on source-line leading whitespace for content indentation inside a
  `\`-continued string literal — the continuation eats it.
- For multi-line embedded content (YAML, shell, etc.) use a **raw string**
  (`r#"..."#`), which preserves indentation verbatim and is far more readable.
- When CI "doesn't run", check `GET /repos/{o}/{r}/actions/workflows` and the
  `action_run` table first: empty workflows means a **detection/parse** failure
  (bad file/indentation/path), not a runner or trigger problem.

## Where this is now documented

- `crates/temper-production/src/provision.rs` (`CI_WORKFLOW` is now a raw
  string with a warning comment + the `ci_workflow_yaml_is_indented_not_flush_left`
  regression test).
- Validated live: `examples/reference-delivery/` converged to a merged,
  reconciled PR with real Forgejo CI after the fix.
