# Harness

Harness is a way to run a software project where AI helpers and people work through the same issue tracker.

The basic idea is simple: an AI helper should not secretly decide what to do next or change project state on its own. It should work the way a teammate would: pick up an issue, open a pull request, wait for CI, respond to review, and move the work forward only when the rules allow it.

Harness provides those rules.

## In plain English

Harness watches Forgejo projects and asks:

- Which issue or pull request needs attention now?
- Who should handle it?
- What are they allowed to do?
- Has CI passed?
- Has review approved it?
- Is it safe to merge?
- If one intake spans repositories, have all child work items landed?

The answers come from normal Forgejo things: issues, pull requests, labels, comments, reviews, dependencies, CI results, and merges.

So the project stays understandable to humans. You can open Forgejo and see what is happening.

## A small example

Someone opens an issue:

```text
Add password reset
```

Harness can move it through a delivery flow like this:

```text
Architect:  turns the request into ready engineering work
Engineer:   claims the work and opens a pull request
CI:         tests the pull request
Reviewer:   approves it or asks for changes
Owner:      merges it when CI and review are both green
```

If CI fails, the work goes back to the engineer.

If review asks for changes, the work goes back to the engineer.

If everything passes, the pull request can be merged.

Nothing magic is hidden away. The issue labels change, the pull request appears, CI runs, review is recorded, and the merge happens in Forgejo.

## Why this matters

AI coding is easier to trust when it is kept inside a clear process.

Harness is built around that idea:

- humans can see the work;
- each helper has limited permission;
- important steps like CI and review are enforced;
- crashed or repeated work can be recovered from;
- the issue tracker remains the shared source of truth.

## What is in this repository?

This repository contains the Rust implementation of Harness, including the workflow runtime, Forgejo support, worker processes, tests, and a reference demo.

The main demo lives in `examples/reference-delivery/`; by default it shows one
intake issue fanning out into work across two repositories.

For more background, read `docs/explanation/agentic-workflows.md`.
