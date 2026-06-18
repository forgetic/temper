# temper-web

The web dashboard for a running temper deployment: a live pipeline **board**
(card lanes + liveness + problem ticker) and a **chat** surface (human ↔ agent
conversations with proposal accept). This crate is the Rust side — it will host
temper-web's own HTTP + SSE server, an in-memory read-model, and the feed
adapters — and `ui/` is the TypeScript front end it serves.

This is the **foundation** (PR 0): the crate skeleton compiles, the day-0 vitest
suite is green, and `npm run build` produces the bundles. The server, read-model,
feeds, and the daemon snapshot endpoint arrive in the fan-out PRs (A–E); see
`docs/plans/temper-web-impl-plan.md`.

## Layout

```
crates/temper-web/
  Cargo.toml                 # workspace member (skeleton; no deps yet)
  src/
    lib.rs                   # thin facade + healthz() placeholder + unit test
  ui/                        # the TypeScript app — its own npm package (temper-web-ui)
    package.json             # pinned exact dep versions; private
    package-lock.json        # committed (CI uses `npm ci`)
    tsconfig.json            # strict, ES2022, bundler resolution
    vitest.config.ts         # node env by default; happy-dom per-file opt-in
    esbuild.config.mjs       # bundles the two entry points -> dist/{board,chat}.js
    .gitignore               # ignores node_modules/ and dist/
    index.html               # board shell -> #root + <script src=dist/board.js>
    chat.html                # chat shell  -> #root + <script src=dist/chat.js>
    styles.css               # design tokens + shell + board/chat CSS (from the mockups)
    src/
      model.ts  view.ts  app.ts          # board MVC (reducer / pure view / controller)
      chat-model.ts  chat-view.ts  chat-app.ts   # chat MVC
      board-main.ts  chat-main.ts        # browser entry points (real seams -> createApp)
      native-event-source.ts             # adapts native EventSource -> EventSourceLike
    test/
      model.test.ts  app.dom.test.ts             # board: reducer (Layer 1/3) + DOM (Layer 2)
      chat-model.test.ts  chat-app.dom.test.ts   # chat:  reducer (Layer 1/3) + DOM (Layer 2)
      fake-event-source.ts                       # ~15-line SSE fake (no network/backend)
    fixtures/
      state-snapshot.json        # GET /v1/state shape (board cold start)
      conversation-events.json   # GET /conversations/{id}/events shape (chat)
```

### The wire contract lives in `ui/fixtures/`

The `ui/fixtures/*.json` files **are** the backend contract — the JSON shapes the
Rust side must emit. They mirror:

- `state-snapshot.json` ← `GET /v1/state` snapshot (board cold start).
- `conversation-events.json` ← `GET /conversations/{id}/events` (chat).
- board deltas (`events/*.json`) are added by the feed PRs.

These shared fixtures are owned by the foundation PR. Feature PRs add **new**
fixture files but coordinate before rewriting the shared ones; Rust-side
serialization tests assert the same shapes so the two sides cannot drift.

## Testing (no browser, no backend)

The UI is tested in three layers, all without a real/headless browser and without
the backend running (see `docs/plans/temper-web-ux.md` Appendix B):

- **Layer 1** — model/reducer + pure derivations, in the plain `node` env.
- **Layer 2** — view + interaction, in happy-dom via Testing Library, driving the
  app with the `FakeEventSource` (per-file `// @vitest-environment happy-dom`).
- **Layer 3** — feed-contract: replay the `fixtures/*.json` through the reducer.

The two seams that make this work: the **injected EventSource**
(`AppDeps.eventSource` / `ChatDeps.eventSource` — tests pass `FakeEventSource`,
prod passes the native one via `native-event-source.ts`) and the **exported MVC**
(`apply` / `render` / derivations are module exports, imported directly by tests).

## Working in `ui/`

```sh
cd crates/temper-web/ui
npm ci          # install from the committed lockfile
npm test        # vitest (34 tests across board + chat)
npm run typecheck   # tsc --noEmit
npm run build   # esbuild -> dist/board.js + dist/chat.js (served as static files)
```

CI runs `npm ci`, `npm test`, and `npm run build` in a dedicated `web` job
(`.forgejo/workflows/ci.yml`), independent of the Rust `validate` job so a TS
failure is legible on its own.

## Server note

temper-web runs its **own** HTTP/SSE server rather than reusing the daemon's
one-shot HTTP responder (`temper-engine-io/src/http.rs`), which cannot hold a
connection open for SSE. The concrete server (serving `ui/dist` + `GET /api/state`
+ the `GET /events` SSE stream) is built in PR D. This foundation carries no
server — only `healthz()` as a stable placeholder.
