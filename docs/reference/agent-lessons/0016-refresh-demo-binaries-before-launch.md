# Lesson 0016: Refresh demo binaries before launch

## Tags

`examples`, `tooling`, `process`, `forgejo`

## Trigger

The reference-delivery demo failed during provisioning with
`temper-provision-forgejo: unrecognized argument '--intake-title'` even though
current source supported that flag.

## What went wrong

`examples/reference-delivery/run.sh` only built launcher binaries when the
executables were missing. An older `target/debug` (formerly `target/release`)
binary could remain on disk after CLI flags changed, so the launcher booted
Forgejo and failed later inside provisioning.

## Steering for future agents

Operator launch scripts that depend on local workspace binaries should either
refresh them before starting long-lived processes or fail early with an explicit
CLI compatibility check. `TEMPER_SKIP_BUILD=1` is an expert mode: assume all
binary paths and overrides are current, and validate stale-binary errors before
starting servers.

## Where this is now documented

`examples/reference-delivery/run.sh` refreshes `temper-production` in the
Cargo development profile by default and checks the provision binary for the
cross-repo intake flags before booting Forgejo. `examples/reference-delivery/README.md`
documents the default refresh and the stale-binary troubleshooting path.
