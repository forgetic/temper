# The web's architecture

Provisional, 2026-10-05. How temper's web is built: a client of temper's
own that runs in a person's browser, its layers, how it meets the browser
and the engine, and how it is tested. What the web does is `ux/`, which
this document takes as given. How its code is written is skein's
`docs/foundation/programming-model.md`, which the client follows as every
temper component does; how it is tested is skein's `testing-strategy.md`,
as `../testing.md` applies it. Why there is no framework is section 8;
what this asks of the rest of temper, section 9; what is open, section 10.

## 1. In one page

- **The web client is a temper component.** It is written in Rust,
  compiled to WebAssembly, and follows programming-model.md like the
  engine, the worker and the agent. There is no framework and no
  JavaScript of temper's own: only the glue wasm-bindgen generates.
- **The browser is its kernel.** The browser's event loop is the client's
  loop. Fetches, server-sent events, timers, storage, the address bar and
  the DOM are its operations, and one shell crate is the only code that
  touches them.
- **Its domain is complete.** Everything `ux/` has the web do (one card
  per object, keyed pending requests, live, behind and offline, paging,
  drafts, folds) is the client domain's, in step functions, with no
  browser in sight, so all of it runs in worlds.
- **Two translations, one per peer.** The engine meets the domain through
  the protocol layer: requests, answers and stream events, as JSON. The
  person meets it through the view: the domain's state as a tree, and the
  person's actions as tokens coming back.
- **The view is a function.** It turns state into a tree of a closed
  vocabulary. A button carries a token, never a closure. Diffing the last
  tree against the next gives patches, which the shell applies.
- **The view tree is the fake DOM.** In every world, the scripted person
  reads the tree by role, name and text and presses what a person would.
  No browser, no WebAssembly and no fake DOM library are involved.
- **A real browser at the top, driven from Rust.** A few scenarios run in
  headless Chromium, driven over the DevTools protocol by a step machine
  in the same loop as the engine: the person's other face.

## 2. The client

```
person ⇄ DOM ┐                        ┌ fetch, server-sent events ⇄ engine
             └──────── shell ─────────┘          the browser's operations
               │                    │
             view               protocol         the client's two translations
               │                    │
               └────── domain ──────┘            what the web does
```

The client has no io layer: the browser reads and writes the bytes, and
hands the shell whole bodies and whole events.

| Crate | What it is |
|---|---|
| `temper-web-wire` | the web protocol's documents and their JSON, used by both ends (section 5) |
| `temper-web-domain` | the client domain (section 3) |
| `temper-web-view` | the tree and its vocabulary, each page's and each card's view, the diff (section 4) |
| `temper-web-protocol` | the client's protocol layer (section 5) |
| `temper-web-shell` | the browser shell, and the bundle's entry point (section 6) |

The first four are step crates under the Rust subset, and build for the
host and for wasm32 alike. The shell builds for wasm32 only, and compiles
to nothing elsewhere, so the workspace's native checks pass over it.

## 3. The domain

The client domain holds what a person's browser knows, and decides what
the web does with it:

- **Objects by id, once.** Each object a card shows (a task as a chip, a
  proposal, an escalation, a question, a person task, a result) is held
  once, and pages name it. Deciding it in one place changes every place
  it shows (ux README, 5.2).
- **Pages:** the address the person is at, what its page shows, the
  snapshot it opened from and the streams it follows, and its windows
  onto lists that are paged (ux README, 5.8).
- **Pending requests:** each made with its key, held until its answer
  says it is durable, sent again with the same key after a drop or a
  reload, and shown as asked, never as done (ux README, 5.3).
- **The page's state:** live, behind or offline (ux README, section 7),
  from its streams and its own deadlines. Behind reloads the snapshot;
  offline keeps what is pending until the engine is back.
- **What the person is in the middle of:** a draft in the composer, a
  reason being written, a tree folded open, a card expanded. The view
  keeps no state, so this is the domain's too, and worlds can see it.

Its inputs are the person's actions (from the view), answers and stream
events (from the protocol), time, and the seed its keys are drawn from.
It is bounded as the engine is: slabs sized at startup, a page's window
onto a list rather than the list, and a full slab a refusal the page
shows, never growth.

## 4. The view

- **State in, tree out.** Each page and each card is a function from the
  domain's state to a tree. A card has one function wherever it shows, so
  one object has one card (ux README, 5.2).
- **A closed vocabulary.** Elements, attributes, roles and the
  stylesheet's classes are enums; text is owned bytes. Every node a
  person acts on has a role and an accessible name, since that is how
  tests find it (section 7.2). The web is accessible because its tests
  need it to be.
- **Bindings are tokens** (programming-model.md, 4.2). A node a person
  acts on carries a token: which action, on which object. The shell
  reports the event with its token, and the view decodes it into a
  domain input. A token whose object has gone is an input like any other,
  and the domain refuses it, as a stale handle is refused.
- **Diffed, not replaced.** The view keeps the last tree. After each step
  that changed what a page shows, the page is built again and diffed
  against it, keyed by objects' ids in lists, into patches: insert,
  remove, move, set text, set an attribute. The patches are the shell's
  requests. A node that can be updated is never replaced, so the browser
  keeps focus, caret, selection, composition and scroll across a patch.
  An input's value is the person's: the view writes it only when the
  domain changes it, as when a composer is cleared once its words are
  sent.
- **Sized from the domain's limits.** A page's window and a card's fields
  bound its tree, so a tree always fits, and a diff's patches are bounded
  by the two trees.
- **Markdown** that agents and people write (turns, results, specs:
  `domain/people.md`, section 11) is turned into tree nodes by the view,
  from a bounded subset, read by a step machine of temper's own. The
  engine renders no HTML.
- **Markup is plain calls** on a tree builder, one node a call, with no
  macros. If whole pages prove unreadable that way, one `macro_rules!` in
  the view crate may expand to the same calls: the one place in temper
  where a macro may live, and only for markup.

## 5. The protocol

- **The web protocol** is `domain/people.md`, section 11: HTTP for
  requests and reads, server-sent events for watches, and JSON both ways.
  Each request carries its key.
- **Both ends are temper's.** `../protocol.md` counted people's browsers
  among the foreign protocols. With a client of temper's own, the web
  protocol is temper's, and its documents are one crate, `temper-web-wire`,
  that the engine's protocol layer and the client's both use, as the
  channel's are one crate (`../protocol.md`, section 1: typed at their
  ends, from one schema). Two ends written together can share a
  misreading, so the wire's decoders are also fed transcripts of real
  exchanges, captured in the browser tier (testing-strategy.md, 4.1).
- **JSON, not the channel's sized format:** server-sent events are text,
  and JSON keeps the browser's own tools useful. `skein-json` is a step
  crate, and builds for wasm32 as it is.
- **Translation, and nothing else** (`../protocol.md`, section 1). The
  domain decides deadlines, retries and what a failure means; the
  protocol layer runs one attempt, and turns answers and stream events
  into domain inputs.
- **In the browser, the browser is the HTTP stack.** The client's
  protocol layer gives the shell a method, a path and a body, and gets
  back a status and a body, or a stream's events. Below the browser, a
  **native shell** carries the same over skein's HTTP client and its
  server-sent events reader, so the worlds that make bytes real run the
  client without a browser (section 7.1).

## 6. The shell

- **The only impure code.** wasm-bindgen, web-sys and js-sys are the
  shell's dependencies alone, as io-uring and libc are the ring
  adapter's. The callbacks the browser needs are the shell's closures,
  never step code's. There is no `async` and no wasm-bindgen-futures: a
  promise completes through a callback, which is one more event.
- **One callback, one step.** Each thing the browser reports (a DOM event
  with its token, a fetch's answer, a stream's event or its end, a timer,
  the address changing) is one event. The shell runs the step, then does
  what it asked: patches, fetches, streams opened and closed, timers,
  storage written, an address pushed. Between two callbacks the client
  is a plain value, as a world is between two iterations.
- **Time and the seed are inputs.** The shell reads the clock
  (`performance.now()` for deadlines, `Date` for wall time) and draws
  the seed from `crypto.getRandomValues`. Request keys are drawn from it,
  since the web page makes them (`domain/people.md`, 5.1.1).
- **Addresses are domain state.** The shell reports where the browser
  went (a load, back, forward), and the domain asks for an address to be
  pushed. A task's address is its number (ux README, section 4).
- **Session storage is the client's small store.** What a reload must not
  lose, its pending requests with their keys and the person's drafts, the
  domain writes through the shell as it changes, and reads at start, and
  then sends what is pending again with the same keys (ux README, 5.3).
  It is never the truth; the engine's store is.
- **Signing in is navigation.** The engine sends the browser to the forge
  and back, and sets a cookie the client cannot read
  (`domain/people.md`, section 3). The client holds no token; an answer
  saying the sign-in is gone takes it to signing in.
- **A panic is fail-stop.** Panics abort, and the shell's panic hook
  reloads the page, as a supervisor restarts a process. The keys make
  what was pending safe to send again.
- **The bundle** is an HTML page, the wasm module, wasm-bindgen's glue,
  the stylesheet and the icons. The engine serves it from the web's one
  origin, so there is no CORS, under a strict content security policy.
  Like configuration, it is in the engine's memory at startup. The page
  is drawn by the client from its first snapshot. The bundle is built by
  cargo for `wasm32-unknown-unknown` and the wasm-bindgen CLI at the
  crate's version: no npm, trunk or dx. The stylesheet starts as the
  mockups', and the view's class enum names its classes.

## 7. Testing

### 7.1 The tiers

| Tier (testing-strategy.md, section 2) | The web in it |
|---|---|
| step tests | the domain's steps; each page's and card's view, state to tree; the diff; the Markdown reader; the wire's documents, against transcripts |
| domain worlds | the client domain joins the engine's world as the people's side: the world translates between the client's requests and the people child domain's, and back; the person acts on the view tree. Most of the web's tests |
| system worlds | the same with the real worker and agents, for what streams from runs |
| protocol worlds | the client's protocol layer, on the native shell, meets the engine's through bytes |
| simulated worlds | the same on skein's simulator |
| real loop | the engine as it ships, the client on the native shell over loopback |
| the browser | the engine as it ships, the real bundle in headless Chromium, driven from the loop (7.3) |

The view is real in every world, domain worlds included. It is a pure
function, costs nothing, and lets each scenario be written once in the
person's terms.

The faults the web adds, drawn from the seed: a stream that drops (the
page falls behind); the engine restarting (offline, then answers by key);
a double press; a reload (the client's own restart, its state gone but
for session storage); two tabs deciding one thing (two clients for one
person, ux README, section 3); a token whose object has since gone.

### 7.2 The person

`../testing.md`, 4.4 scripts people in ordinary Rust, and has them grow
the web protocol's client side. With a client of temper's own, the
person moves up a layer: they act on the client, as a person does, and
the protocol is the client's. One person, two faces (testing-strategy.md,
section 4):

- **The tree face,** in every tier below the browser: it finds nodes in
  the view tree by role, accessible name and text, presses them and
  types into them. It never finds them by class, by position or through
  the client's state.
- **The DOM face,** in the browser: the same queries over Chromium's
  accessibility tree, the same actions as input events.

A scenario is written once, in a person's terms ("open the inbox; press
Accept on T1's proposal; see it gone from the chat"), and runs at every
tier. The referee watches what the person sees and the facts the engine
emits, never the client's state (testing-strategy.md, section 7).

### 7.3 The browser tier

- **Chromium, headless, over a pipe.** Launched with
  `--remote-debugging-pipe`, it speaks the DevTools protocol (CDP) as
  JSON documents, each ending in a NUL byte, on the child's descriptors 3
  and 4. There is no WebSocket, WebDriver or Node.
- **The driver is a step machine** over skein's io (the spawned child and
  its pipes) and `skein-json`, in the real loop beside the engine: one
  thread, no async runtime, no CDP crate. It speaks only the part of CDP
  the DOM face needs: open a page, query the accessibility tree, dispatch
  input, read console messages and exceptions, and take a screenshot
  when a test fails.
- **What only a browser shows:** the shell (patches keep focus, caret,
  selection and scroll; events carry the right tokens; session storage
  survives a reload; a panic reloads) and the bundle (it loads under its
  content security policy, and signs in through the fake forge). Then a
  few journeys of the first slice (ux README, section 9): sign in, start a
  chat, decide a held chat's escalation.
- **A suite of its own.** It needs Chromium, does not replay, and pays
  for a browser's start, so it is neither focused nor fuzzy. The gate
  runs it, and also builds the bundle and runs clippy for wasm32.

## 8. Why no framework

The candidate was Dioxus: of the Rust frameworks, its native DOM made it
the strongest for testing without a browser. Leptos and Yew share the
first two objections.

- **Its model is the opposite of programming-model.md's.** It is built on
  closures, signals (shared state with interior mutability), hooks, async
  tasks and procedural macros. The view could not follow the Rust
  subset, and the framework pulls state into components, out of the
  domain, where worlds do not see it.
- **It brings its own scheduler,** a second loop in every world, against
  "one thread, one loop, in every world".
- **Its fake DOM does not come apart from it.** `dioxus-native-dom` hosts
  a Dioxus VirtualDom, so testing with it means building with Dioxus.
- **It is still moving.** As of 2026-10 it is at 0.8 alpha, which changes
  signals again, and its native DOM only just gained synthetic clicks. Its
  dependency tree is heavy, against temper's build times.

What it would give: markup in `rsx!`, and a DOM patcher that is already
proven. What temper writes instead: the tree builder, the diff, the
shell's patching, and their care for focus and input values, all tested
in the browser tier.

The choice stays reversible. A framework could render the same tree, and
the domain, its worlds and its scenarios would not change.

## 9. What this asks of the rest of temper

- **The engine:** the web protocol's server side in its protocol layer,
  over `temper-web-wire`; serving the bundle; the reads and streams of ux
  README, section 10.
- **`../protocol.md`:** the web protocol moves from the foreign
  protocols to temper's own (section 5).
- **`../testing.md`:** people act on the client (7.2); the browser as a
  tier (7.3); the layout gains the native shell, the person's two faces
  and the CDP driver, under `testing/`.
- **skein:** a spawned child with two more pipes, at descriptors 3 and 4,
  for the driver. The native shell needs only what skein has: the HTTP
  client and the server-sent events reader.
- **The toolchain:** the standard library for `wasm32-unknown-unknown`
  (the development machine's rustc is Debian's, without rustup, so
  `libstd-rust-dev-wasm32`) and the wasm-bindgen CLI. Chromium is
  installed.

## 10. Open questions

- **The browser suite's budget:** proposed at 30 seconds, set when its
  first tests are built.
- **A markup macro,** if plain calls prove unreadable (section 4).
- **Which Markdown:** the subset the view reads, kept to what agents
  write, and what it does with the rest (shown as text, never dropped).
- **Other browsers:** Firefox and WebKit through WebDriver BiDi, if a bug
  ever shows only there.
