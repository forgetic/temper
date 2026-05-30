# Run the Forgejo end-to-end harness

> **Status: in progress.** This page is built up across the Forgejo e2e phases
> (`plans/forgejo-e2e/`). Phase 1 landed the throwaway-server harness; the
> multi-process test itself arrives in a later phase. Until then this documents
> the server harness and its smoke test.

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

## The pinned binary

The first gated run downloads a pinned Forgejo binary into `.cache/forgejo/`
(gitignored) and verifies its SHA-256 before use; later runs reuse it.

| Field | Value |
| --- | --- |
| Version | `7.0.12` |
| Platform | `linux-amd64` (SQLite, statically linked) |
| SHA-256 | `ecd25535250aeb8073fdef1a0c9e92f288de1c0cdde24c95a3b61ead6bc9cf7c` |
| Source | `https://codeberg.org/forgejo/forgejo/releases/download/v7.0.12/forgejo-7.0.12-linux-amd64` |

### Environment knobs

| Variable | Effect |
| --- | --- |
| `HARNESS_FORGEJO_E2E=1` | opt in (required); without it the tests no-op |
| `HARNESS_FORGEJO_BINARY` | absolute path to a pre-downloaded binary; skips the download and checksum (operator vouches for it) |
| `HARNESS_FORGEJO_VERSION` | override the pinned version in the default download URL |
| `HARNESS_FORGEJO_URL` | override the download URL (checked only when paired with `HARNESS_FORGEJO_SHA256`) |
| `HARNESS_FORGEJO_SHA256` | override the expected checksum |

A mismatched checksum fails loudly; the partial download is never published to
the cache path.

## Why blocking HTTP in the harness

The harness downloads the binary and polls readiness with a **blocking**
`reqwest` client. The async `harness-forge-forgejo` backend needs a Tokio
reactor, so driving it against the live server happens in the multi-process test
(under an async runtime), not in the Phase 1 lifecycle code.
