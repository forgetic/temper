# Lesson 0018: Snapshot long-running shell launchers

## Tags

`examples`, `process`, `shell`, `teardown`

## Trigger

A reference-delivery demo that had been running while `run.sh` was edited later
hit `Syntax error: Unterminated quoted string` exactly as its `run-secs` backstop
started teardown.

## What went wrong

`dash`/POSIX `sh` can read script bodies lazily. Editing or replacing a
long-running shell script while it is sleeping can leave the running interpreter
reading a tail from a different file version, producing a parse error unrelated
to the code path that was originally launched.

## Steering for future agents

For blocking demo launchers that may run while agents patch the repository,
start from a private runtime snapshot (or do not edit the script until the run is
stopped). Keep teardown idempotent so even a parse failure or force-stop leaves a
clear recovery path.

## Where this is now documented

`examples/reference-delivery/run.sh` snapshots itself for `start` runs and cleans
up snapshot files during teardown.
