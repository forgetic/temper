# Phase 5 — Optional Matrix/mobile text adapter

This phase is intentionally optional. Run it only after the CLI MVP and local
service API have been dogfooded enough to show that Matrix is worth using as an
early mobile text surface.

## Bootstrap

1. Follow the normal session bootstrap in `AGENTS.md`.
2. Read:
   - `plans/product-manager-chat/README.md`
   - Phase 4 service API docs
   - Matrix SDK/bot docs relevant to the implementation language selected
3. Confirm with the human which Matrix homeserver, bot account, and room/DM
   policy to target.

## Goal

Add a Matrix text adapter that gives immediate Android access through existing
Matrix clients, while keeping Forgejo as transcript/issue backend and the Phase 4
service/core as the authority.

## Scope

In scope:

- receive Matrix messages addressed to the product-manager bot;
- forward messages to the product-manager conversation core/service;
- send product-manager replies back to Matrix;
- support a simple file command, e.g. `file <slug>` or `file 1`;
- mirror everything to Forgejo exactly as the CLI/service already do.

Out of scope:

- Matrix widgets;
- MatrixRTC/live voice;
- rich draft cards beyond markdown text;
- replacing the web/PWA path.

## Design notes

Prefer making Matrix a thin adapter over the Phase 4 API rather than another
copy of the Forgejo/LLM logic. That keeps external UIs, CLI, and Matrix aligned.

Decide whether the Matrix adapter belongs in this repo or another repo at phase
start. If it only consumes the Phase 4 API and has no Temper-internal coupling,
it may belong outside this repository.

## Acceptance criteria

- A Matrix user can discuss product ideas with the product-manager from Android.
- Replies and filed issues match the CLI semantics.
- Forgejo transcript/filing remains the source of truth.
- No voice or rich web UI is added in this phase.
