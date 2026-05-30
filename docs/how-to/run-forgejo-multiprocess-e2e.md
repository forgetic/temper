# Run the Forgejo end-to-end harness

> **Status: in progress.** This page is built up across the Forgejo e2e phases
> (`plans/forgejo-e2e/`). Phase 1 landed the throwaway-server harness and Phase 1b
> added a real host-mode `forgejo-runner`; the multi-process test itself arrives
> in a later phase. Until then this documents the server + runner harness and
> their smoke tests.

This harness runs a **real Forgejo** server locally so the reference-delivery
workflow can be exercised against the `harness-forge-forgejo` backend instead of
the in-memory/filesystem backends. Like the `harness-forge-forgejo` live tests,
everything here is `#[ignore]`d **and** gated behind an environment variable, so
a plain `cargo test` never downloads a binary or opens a socket.

## Smoke test (Phase 1)

```sh
HARNESS_FORGEJO_E2E=1 \
  cargo test -p harness-testing --test forgejo_server -- --ignored
```

This boots a throwaway Forgejo on an ephemeral port against a fresh SQLite data
dir, polls `/api/v1/version` to readiness, and kills the process plus removes the
data dir on drop.

## Runner smoke test (Phase 1b)

```sh
HARNESS_FORGEJO_E2E=1 \
  cargo test -p harness-testing --test forgejo_runner -- --ignored
```

CI is **real**: this boots the server plus a host-mode `forgejo-runner`
(`--labels host:host`, **no containers**), provisions a repo whose
`.forgejo/workflows/ci.yml` deliberately fails (`run: exit 1`), and polls the
head commit's status API until the real runner reports `state: "failure"`. Both
the server and the runner daemon are killed and their temp dirs removed on drop.

The server config enables Actions (`[actions] ENABLED = true`). Provisioning here
uses an admin token minted via the server CLI (`admin user create` then
`admin user generate-access-token --scopes all --raw` — a plain `--access-token`
yields a scopeless token that 403s on 7.0.x). Reading CI via the password/web-UI
live-view JSON is Phase 3b; commit status is the cheap confirmation here.

The runner spawns real OS processes and executes jobs **on this host**, so run it
only where that is acceptable.

## The pinned binaries

The first gated run downloads the pinned Forgejo **and** `forgejo-runner`
binaries into `.cache/forgejo/` (gitignored) and verifies each SHA-256 before
use; later runs reuse them.

| Binary | Version | SHA-256 | Source |
| --- | --- | --- | --- |
| Forgejo server | `7.0.12` | `ecd25535250aeb8073fdef1a0c9e92f288de1c0cdde24c95a3b61ead6bc9cf7c` | `https://codeberg.org/forgejo/forgejo/releases/download/v7.0.12/forgejo-7.0.12-linux-amd64` |
| `forgejo-runner` | `3.5.1` | `e2f36aa8149a0e883b5713398aa185c88a827fc0527d5cd2e2b05b88c9ba0b36` | `https://code.forgejo.org/forgejo/runner/releases/download/v3.5.1/forgejo-runner-3.5.1-linux-amd64` |

Both are `linux-amd64` (the server is SQLite, statically linked).

### Environment knobs

The server uses the `HARNESS_FORGEJO_*` namespace; the runner mirrors it under
`HARNESS_FORGEJO_RUNNER_*`.

| Variable | Effect |
| --- | --- |
| `HARNESS_FORGEJO_E2E=1` | opt in (required); without it the tests no-op |
| `HARNESS_FORGEJO_BINARY` | absolute path to a pre-downloaded **server** binary; skips its download and checksum (operator vouches for it) |
| `HARNESS_FORGEJO_VERSION` | override the pinned server version in the default download URL |
| `HARNESS_FORGEJO_URL` | override the server download URL (checked only when paired with `HARNESS_FORGEJO_SHA256`) |
| `HARNESS_FORGEJO_SHA256` | override the expected server checksum |
| `HARNESS_FORGEJO_RUNNER_BINARY` | absolute path to a pre-downloaded **runner** binary; skips its download and checksum |
| `HARNESS_FORGEJO_RUNNER_VERSION` | override the pinned runner version in the default download URL |
| `HARNESS_FORGEJO_RUNNER_URL` | override the runner download URL (checked only when paired with `HARNESS_FORGEJO_RUNNER_SHA256`) |
| `HARNESS_FORGEJO_RUNNER_SHA256` | override the expected runner checksum |

A mismatched checksum fails loudly; the partial download is never published to
the cache path. A sandboxed/offline machine should point the two `*_BINARY`
overrides at pre-downloaded binaries.

## Why blocking HTTP in the harness

The harness downloads the binary and polls readiness with a **blocking**
`reqwest` client. The async `harness-forge-forgejo` backend needs a Tokio
reactor, so driving it against the live server happens in the multi-process test
(under an async runtime), not in the Phase 1 lifecycle code.
