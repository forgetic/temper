# temper-web

The web dashboard for a running temper deployment: a live pipeline **board**
(card lanes + liveness + problem ticker) and a **chat** surface (human ↔ agent
conversations with proposal accept). This crate is the Rust side — it will host
temper-web's own HTTP + SSE server, an in-memory read-model, and the feed
adapters — and `ui/` is the TypeScript front end it serves.

PR D adds the Rust server: the in-memory read-model, the SSE board feed, the
log-tail adapter, lane derivation, and a `std::net` HTTP server that serves the
UI bundle plus `GET /api/state` and `GET /events`. The chat conversation SSE
proxy (feed 2) and the live agent token stream (feed 3) are later PRs; see
`docs/plans/temper-web-impl-plan.md`.

## Layout

```
crates/temper-web/
  Cargo.toml                 # workspace member: serde, temper-log, temper-workflow
  src/
    lib.rs                   # facade + module map + healthz()
    config.rs                # WebConfig — explicit config (no ambient env in lib)
    board.rs                 # board wire types (the model.ts cross-language contract)
    logsource.rs             # FileLogSource — tails a temper-log JSON-lines file
    readmodel/               # in-memory board projection (cards/workers/problems) + seq
    project/
      snapshot.rs            # daemon raw /v1/state -> board {t:snapshot,...} envelope
      lanes.rs               # lane derivation from the exclusive lifecycle state dimension
    feeds/
      logtail.rs             # feed 1b: temper-log JSON lines -> board deltas (by artifact.ref)
      snapshot_source.rs     # feed 1a: cold-start snapshot source seam (daemon/fixture/empty)
    server/
      mod.rs                 # AppState + route + serve (blocking TcpListener, thread/conn)
      sse.rs                 # SSE framing + Broadcaster fan-out (PR E reuses this)
      static_files.rs        # static UI file resolution + content types
      request.rs             # minimal blocking HTTP/1.1 request reader + response writer
    bin/
      temper-web.rs          # the server binary (composition root; the env boundary)
      temper-web/daemon_client.rs  # tiny blocking HTTP client for the daemon snapshot
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

- `state-snapshot.json` ← the board cold-start `{t:"snapshot",…}` envelope.
- `conversation-events.json` ← `GET /conversations/{id}/events` (chat).
- `events/*.json` ← individual `temper-log` JSON lines (the log-tail adapter's
  input: `transition.applied`, `ci.completed`, `pr.merged`, `lease.lost`,
  `role.saturated`, `agent.started`). Both the Rust adapter tests and the TS
  reducer read these, so the two sides can't drift.

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

## The Rust server (PR D)

temper-web runs its **own** HTTP/SSE server rather than reusing the daemon's
one-shot HTTP responder (`temper-engine-io/src/http.rs`), which cannot hold a
connection open for SSE. It is a **dependency-light blocking server**: a plain
`std::net::TcpListener` with a thread per connection (no skein, no async runtime
— matching the workspace's other non-daemon servers), extended to keep `/events`
connections open and stream `text/event-stream` frames. This keeps temper-web a
depgraph leaf.

### Endpoints

- `GET /` → `index.html`; `GET /chat` → `chat.html`; other paths map onto files
  under the UI dir (`/styles.css`, `/dist/board.js`, …).
- `GET /api/state` → the board cold-start `{t:"snapshot",seq,state:{…}}` envelope,
  projected from the daemon's raw `GET /v1/state` (or an empty/fixture board when
  no daemon is configured, so the server runs standalone).
- `GET /events` → the SSE board stream: a snapshot greeting then live deltas,
  each carrying a monotonic `seq`, with `: keep-alive` comments (~15s) so the
  client's pulse tells idle from dead. On reconnect the client re-snaps and
  resumes from the cursor — no gap, no dup.
- `GET /healthz` → `ok`.

### Feeds

- **Feed 1a (snapshot):** `feeds::snapshot_source` — injectable cold-start source
  (live daemon HTTP, a fixture, or empty), projected by `project::snapshot`.
- **Feed 1b (log-tail):** `feeds::logtail` parses `temper-log` JSON lines and
  maps the event vocabulary to board deltas joined by `artifact.ref`. The
  `LogLineSource` trait is the swap seam: `FileLogSource` tails the JSON log
  today; a future in-process `temper-log` broadcast seam swaps in unchanged.

### Lanes (UX Appendix A.4)

Board columns come from a workflow's **exclusive lifecycle state dimension**
(`project::lanes`), not queues — queues (`pr_changes_requested`, `pr_ci_failed`,
`needs_owner`, role saturation) are **problem/attention overlays**. The
arbitrary lifecycle states are mapped onto the board's fixed five lanes
(`triage`/`implement`/`review`/`ci`/`done`) by a name heuristic with an ordered
positional fallback; a workflow with no exclusive lifecycle dimension is logged
(not guessed silently) and falls back to a queued/in-flight default.

### Config (no ambient env in library code)

All configuration is an explicit `config::WebConfig` (bind addr, UI dir, optional
daemon URL, optional log path). The server binary (`src/bin/temper-web.rs`) is
the sole env boundary — it snapshots argv/env into the config and threads it in;
library code never reads `std::env`.

```sh
cargo run -p temper-web --bin temper-web -- \
  --bind 127.0.0.1:8080 --ui-dir crates/temper-web/ui \
  --daemon-url http://127.0.0.1:9000 --log-path /var/log/temper.jsonl
```
