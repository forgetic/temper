# The forge's setup

Provisional, 2026-10-16, revised 2026-10-23. How a forge comes to be
ready for temper, in three modes: connected, where a person runs and sets
up the forge and gives temper only what it strictly needs; managed,
where a person runs it and temper sets itself up on it, with a Forgejo
site admin's token or, on GitHub, with an App made from temper's
manifest; and owned, where temper runs Forgejo and its runner, and sets
itself up on them. A fourth, fake, is the demo's: temper runs its own
fake Forgejo, seeded with a demo repository, and sets itself up on it as
on an owned one (8.4). A person chooses a forge's mode and connects it from
temper's client (`../client/forge.md`, the deployment's page). The
engine is the same in every mode: each mode makes the records the one
below reads, kept in jig's secrets' store (credentials.md, the records),
and the engine's forge component reads connected's. Owned mode is
commands any host can arrange; `temper box` is one arrangement, and
wiring only. A box is what temper's first milestone is set up on and
checked by CI on (`../README.md`, section 2), and what QA's agents drive
(`../draft/qa.md`, 4.1). The members are configuration.md's (section 3),
the credentials credentials.md's (section 2), the webhooks forge.md's
(section 5), and the commands and crates README.md's (sections 4 and 5).
A person's walks are `docs/guide/box.md`, `docs/guide/forgejo.md` and
`docs/guide/github.md`. What comes later is section 15; what is open,
section 16; connecting GitHub, section 17.

## 1. In one page

- **Three modes, per forge** (section 2), chosen and connected from
  temper's client. Connected: a person runs and sets up the forge, and
  pastes the bot's two tokens on Forgejo, or an App's id and key on
  GitHub. Managed: a person runs it, and pastes a Forgejo site admin's
  token, or on GitHub makes temper's App from its manifest in their
  browser. Owned: temper runs Forgejo and its runner. A fourth, fake,
  is the default document's and never chosen: temper runs its fake
  Forgejo in place of Forgejo, with no runner and its git a route of its
  own, and sets it up as an owned one (8.4).
- **Each mode is a layer that makes the inputs of the one below.**
  Owned's preparation makes a Forgejo and an admin's token, managed's
  input, and hands the token over in a file; managed's plans make the
  bot, its tokens, its id and the webhooks' secret, connected's inputs.
  The forge component always runs connected: it reads the same records,
  checks the same user, and never holds an admin's token (section 3).
- **The records are the interface.** Each forge's records, under its
  configured `number`, a stable id never reused, are kept in jig's
  secrets' store by the engine's protocol layer alone (credentials.md,
  the records). The one file that passes between the layers is an owned
  forge's handover, `admin_token`, which the engine takes once.
- **The core is step code; the hosts are thin.** The setup's plans are
  skein's `skein-forge-setup` (skein's `forge-setup.md`, sections 4 and
  5), run inside the engine by skein's setup host component (its host,
  6.3): per forge, one writing plan at once and the picker's read beside
  it (5.1). The preparation, Forgejo's settings and the processes' specs
  are machines and pure functions of `temper-forge-setup`, hosted by
  three commands, `temper forge fetch`, `temper forge prepare` and
  `temper forge run` (section 6). No command writes the records.
- **`temper box` is wiring only.** It runs a configuration's steps in
  order (the fakes, fetch, prepare, Forgejo, its runner, `temper
  serve`), restarts what exits, and stops them in reverse order. An
  owned forge's steps, and the demo's fakes, may wait, dormant, until
  the client asks for them over a pipe to `temper serve` (7.1). It names no
  Forgejo request and nothing of setup but its steps. systemd, QA's
  environments or a test arrange the same commands another way (section
  7).
- **`temper init` writes a deployment that starts** into the three
  directories (configuration.md, section 12): the default document, its
  charters, its agent's configuration, the worker's secret and the demo
  model's key; no account and no credential, which are the client's.
  `temper box` runs it first on an empty configuration directory (8.2,
  8.3).
- **A first run is a demo** (8.4): the default document's fake LLM and
  fake forge are the box's steps, dormant until the person chooses the
  playground; the engine's start of the demo then asks for them, adds
  the demo model's key and adopts the demo repository, as the person
  would. Real backends are added from the client beside it: GitHub
  and an owned Forgejo are in the default document, so connecting
  either needs no restart, and the owned one's steps wait until the
  client asks for its setup (8.3, 7.1).
- **Reconcile converges at every start** of a managed or owned Forgejo,
  in the engine: the bot user, made when missing and otherwise checked,
  never edited; its two tokens; and each
  adopted repository's access, the bot a collaborator with write and the
  repository's hook; each made when missing, repaired when changed, and
  left alone otherwise (section 5).
- **Access at adoption.** Adopting a repository of a managed or owned
  Forgejo first gives the bot its access (`Adopt`, 5.4); nothing is
  granted org-wide. The picker offers every repository the site admin
  can see, read by the `Reachable` plan, which writes nothing (5.2).
- **GitHub** (section 17): managed by the manifest flow, through the
  clients' listener's routes, or connected by an App the person pasted;
  both end in the same records. Its reconcile, after each connect,
  repairs only the App's hook's address; its changes reach the engine
  through the App's delivery log, with no webhook secret.
- **Owned is a stock Forgejo.** temper writes its `app.ini`, runs its
  migrations, makes its own admin and the person's account with
  Forgejo's command line, registers the runner offline, dumps before an
  upgrade's migration, and runs only the versions it pins, the
  conformance's, which it fetches itself when asked, checked against
  the digests it pins (section 6). Its directory, once temper stops running
  it, is a Forgejo a person can run themselves, as managed.
- **A box is reactive.** Every change on the forge that a box's work
  waits on reaches the engine as a signed webhook delivery, and becomes a
  hint, read within the keep's `hinted`: pushes, pull requests, issues,
  comments, reviews and each CI job's end. Adoption and reconcile make
  the hooks; an owned Forgejo is allowed to deliver to the engine's
  listener. Polling is unchanged, the fallback for a delivery lost or
  refused (section 10).
- **Loopback.** An owned Forgejo listens on loopback, as temper's
  clients' and webhooks' listeners do; a person on another machine
  reaches them through a tunnel. A forge on another host polls: its
  webhooks need skein's TLS server side (section 15).
- **Not here:** webhooks from another host, the isolation of CI's jobs,
  provisioning at runtime and the forge as people's sign-in provider
  (section 15).

## 2. The modes

| | connected | managed | owned | fake |
|---|---|---|---|---|
| Runs the forge and CI's runner | the person | the person | temper | temper: its fake forge, `temper fake forge`, whose CI answers each push itself; no runner |
| Makes temper's bot, its tokens, its access and the webhooks | the person | temper: on Forgejo with a site admin's token; on GitHub, an App from temper's manifest | temper, with an admin it makes | temper, with the admin the fake is seeded with |
| temper is given, on Forgejo | the bot's two tokens, pasted; optionally a webhooks' secret | a site admin's token, pasted | the Forgejo and runner binaries, or `"fetched"` for temper's (6.6); an address on loopback; the person's login and email | an address on loopback; the seed's owner and repositories |
| temper is given, on GitHub | an App's id and its private key, pasted | the person's steps in their browser, from a link temper prints | none: GitHub is never owned | none: the fake is a Forgejo |

- **Fake is owned's layers,** with temper's fake forge in place of
  Forgejo and its runner (8.4). What this document and the others say
  of an owned forge's handover, directory, reconcile, records, adoption
  and picker holds of a fake one; what they say of Forgejo's or the
  runner's binaries, `app.ini`, the preparation's command line and the
  person does not. A fake forge
  keeps nothing across its starts but its directory's handover: each
  start serves the seed afresh, and reconcile makes the bot again
  (8.4).
- **Per forge.** A deployment may mix them: an owned Forgejo beside a
  connected one, or beside GitHub, or the demo's fake beside a person's
  real forges. Each owned or fake forge has a directory and an address
  of its own.
- **A mode is no record** (credentials.md, the records): a forge is
  owned or fake when its `setup` entry says so (section 8); managed when
  it is neither and its `AdminToken` record is kept; connected otherwise. On
  GitHub, managed and connected end in the same records, and differ only
  in how the App came to be (section 17).
- **A mode changes:**
  - owned to managed, by the configuration and a restart: the forge's
    `setup` entry says `"managed"`, temper stops running Forgejo, and
    its directory's `forgejo/` is a stock work path a person runs; the
    `AdminToken` record, taken over (6.5), stays valid;
  - managed to connected, from the client, "Forget the admin's token":
    the `AdminToken` record is erased, no reconcile runs for the forge,
    and its bot, tokens, id and hooks stay as reconcile last kept them;
    adopting a repository then grants nothing, and the person makes the
    bot a collaborator (section 4);
  - connected to managed, from the client, "Connect as its admin": a
    site admin's token is pasted, and reconcile runs with it, taking
    over the connected user as its bot when its full name carries
    `temper:bot` (5.5).

## 3. The layers and their files

The three modes and their layering (each mode makes the inputs of the
one below, and the service always runs connected) are held by skein's
`forge-setup.md`, section 3; fake is owned's layer with another forge in
it (section 2). temper's layers, and what passes between them:

```
owned      temper forge fetch        ──► the Forgejo and runner binaries, in the cache (6.6)
           temper forge prepare      ──► the forge's work path, the runner's, person_password,
                                         admin_token (the handover)
           temper forge run          ──► forgejo web (its child on Linux), forgejo-runner daemon (exec'd)
fake       temper forge prepare      ──► the forge's directory, admin_token (the handover)
           temper fake forge         ──► the fake, seeded with temper-admin and that token (8.4)
managed    the engine's setup host   ──► the forge's records (below); takes the handover once
connected  the forge component       ◄── its access, built from the records
```

- **One directory per owned or fake forge,** named by its setup (section
  8): private, `0o700`, the user's own and not reached through a link,
  as a worker's root is (configuration.md, section 4). `temper forge
  prepare` makes it so when it is absent; a directory that is not so
  refuses the command, naming it. A managed or connected forge has none.
  A fake forge's holds `admin_token` until the engine takes it; `temper
  fake forge` reads it before binding (8.4). An owned forge's files:

  | File | Made by | Read by | Holds |
  |---|---|---|---|
  | `forgejo/` | prepare | Forgejo, through `run` | Forgejo's work path: `app.ini`, its database, repositories and logs |
  | `runner/` | prepare | the runner, through `run` | the runner's configuration, its registration, its jobs' directories, `secret` |
  | `backups/` | prepare | the person | one dump before each upgrade's migration |
  | `state` | prepare | prepare, `run` | owned's record: the versions prepared, whether the first preparation ended, Forgejo's own secrets |
  | `admin_token` | prepare, when missing | the engine's setup host, once, at start or the box's `started` (6.5) | the handover: temper's admin's token, kept as the forge's `AdminToken` record by `TakeOver`, after whose `Done` the file is removed (6.5) |
  | `person_password` | prepare | the person | the person's first password on the owned Forgejo |

- **What the preparation leaves for the engine** is `admin_token`
  alone: the token's text, nothing else, private and written whole;
  made at the first preparation and at each later one that finds it
  missing, which it is once the engine has taken it, and left as it is
  when present (6.3).
- **Fetched binaries are the cache's,** not the forge's directory's:
  `<cache>/forgejo/VERSION/forgejo` and
  `<cache>/forgejo-runner/RUNNER_VERSION/forgejo-runner`, shared by
  every owned forge whose binaries are `"fetched"`, written by `temper
  forge fetch` alone and fetched again at will (6.6; configuration.md,
  section 12).
- **No other credential file is written or read for owned or fake.**
  Their `api_token`, `git_token`, `user` and `webhook_secret` files,
  including any left by an earlier temper, stay unread. Each record is
  written by a plan, `User` as its 8 bytes within a version-1 record
  (credentials.md, the records). The managed and connected old forms are
  transitional until temper-15 (6.5).
- **Written whole.** Each file is written beside its name and renamed
  over it, private, so a reader never sees half a token.
- **The handover is read** as configuration.md, section 6 reads a
  secret's file, within `records.value_bytes`: it is the one credential
  file of an owned or fake forge that `temper serve` reads (6.5).
- **The records, by mode** (credentials.md, the records):

  | Forge | Records |
  |---|---|
  | Forgejo, connected | `ApiToken`, `GitToken`, `User`, `Origin`; `WebhookSecret` when the person gave one |
  | Forgejo, managed, owned or fake | those, and `AdminToken`; `WebhookSecret` drawn by reconcile when the webhooks' listener is configured |
  | GitHub, managed or connected | `AppId`, `AppSlug`, `AppKey`, `User` (the App's bot user), `Accepted`, `Origin`; no `WebhookSecret` |

- **Written by the setup host's plans alone,** each write kept before
  the plan goes on (5.1). A value a person pastes is kept by jig until
  its run ends (credentials.md, pasted values), and is no record.
- **Records made for another forge,** an `Origin` naming another kind
  or address, and unreadable ones, leave that forge not connected,
  named on the deployment's page; nothing is erased, and connecting
  again overwrites them (credentials.md, the records).
- **Nothing passes between the layers** in an argument or the
  environment but the configuration's path and the forge's number.

## 4. Connected

- **What temper strictly needs** of a connected Forgejo: the bot's API
  token and its git token, and, for webhooks, their secret; the bot's
  id is read with the tokens (credentials.md, sections 4 and 5). The
  scopes (skein's `forge-setup.md`, section 8): `read:user`,
  `write:issue` and `write:repository` for the API token;
  `write:repository` and `read:user` for the git token, the second so
  that the setup's plans can ask whose it is (skein's `forge-setup.md`,
  4.3, step 3, and 4.6).
- **What the person does:** a Forgejo user for temper, not a site admin;
  its two tokens; its write access to each repository temper will adopt,
  as a collaborator or through a team; optionally, each repository's or
  org's webhook to temper's listener, `http://<listener>/hooks/<number>`,
  with the secret and the events of skein's `forge.md`, section 7, and
  Forgejo's `[webhook] ALLOWED_HOST_LIST` letting it reach the listener;
  CI, as each repository has it.
- **Pasted from the client, together** (`Connect` with `ForgejoTokens`;
  `../client/forge.md`, the deployment's page): both tokens, and the
  secret when given, one value, kept by jig until its run ends
  (credentials.md, pasted values). The setup host's `Check { forge }`
  (skein's `forge-setup.md`, 4.6) finds that both answer as one user,
  not a site admin, and writes that user, and the routing keeps the
  tokens and the secret as the forge's records; the page then says
  "Connected as LOGIN", with its id on an Account line (6.5). Tokens of
  two users stop it `OtherUser`, a site admin's `BotIsAdmin`, a refused
  token `Refused`, each named, and the forge stays as it was.
- **Replaced** from the client (`Replace`): a new token, checked to
  answer as the kept user, kept before the old is erased.
- **Nothing more is required** (`../domain/forge.md`, section 4): polling
  stands in for webhooks, and a project's checks for CI.
- **A connected GitHub** is an App the person made, pasted (17.3).

## 5. Managed: reconcile

Reconcile is skein's plan, run by the engine's setup host at each start
of a managed, owned or fake Forgejo, at a managed connect and at an
owned forge's set up from the client (5.1). The managed and connected
old forms alone keep their transitional command until temper-15 (6.5);
otherwise nothing but the setup host runs it or writes what it makes.

### 5.1 The setup host in the engine

- **skein's host component** (skein's `forge-setup.md`, its host, 6.3),
  a child of the engine's protocol layer beside the forge component; the
  routing carries what passes between it, the root and the records
  (forge.md, the setup's host). It holds an admin's token and a GitHub
  App's key only while a plan that asked for them runs, and drops them
  at its end.
- **Per forge, one writing plan at once, and one `Reachable` beside
  it,** since the picker's read writes nothing. A second of either kind
  is refused "busy", never queued, and so is one past `setup.runs`,
  which counts both kinds. One exception: an `Install` waits for the
  engine's own run on its forge (the reconcile after a connect, at a
  start or on a hint), at most one per forge and for at most
  `setup.state_life_ms`, and is refused busy only by a run a person
  asked, since its link writes nothing.
- **Its needs:** a write is a `Keep` of the forge's record, answered
  `Written` only once `Kept`, so a browser is answered and a connect
  sent only on what a restart would find; a read is served from the
  run's pasted value first, then from the forge's record. It draws its
  random bytes from the engine's source.
- **States,** for GitHub's browser steps, are its own, in memory: at
  most `setup.states`, each living `setup.state_life_ms`, an hour at
  most; a restart forgets them (17.2).
- **At each start,** once the records are loaded and the root restored:
  an owned or fake forge's handover by `TakeOver` whenever its file is
  present, even with a kept `AdminToken`, its file removed, then
  `Reconcile` with the restored repositories and connect (6.5); each
  managed Forgejo's reconcile; the fake's start-time adoption (8.4); and
  GitHub's `Reconcile` after each GitHub forge connects (17.5). The
  deployment's page shows a forge "Connecting" until its `Connected`.
- **At the box's `started`,** the setup host runs the same handover,
  removal, reconcile and connect sequence for the forge (6.5). A stop or
  failed removal leaves it set up failed, named by the step, and offered
  again; every other forge runs on.
- **A run's end:** `Done`, or `Lacking` (done, with what the App lacks
  named), keeps its writes, then connects the forge after a plan that
  connects; `Stop` names its failure and its step, with no secret, and
  leaves the forge as it was. A start's reconcile that stops leaves that
  forge not connected, named, until the next start, a managed connect or
  an owned forge's next set up ask (6.5); the engine and every other
  forge run on.

- **Closing progresses without diagnostic consumption.** The service's
  setup adapter tracks closing requested separately from host `Close`
  sent. A close request enters closing once, even when active work's
  room gates are full. While closing it drains the host's outputs
  independently of notes, store, forge and events room, discarding
  non-`Closed` outputs as the shutdown path does. It sends `Close` once
  bounded host room permits, at most once, and continues io settlement
  and draining until the actual `Closed`. Neither requesting close nor
  sending it requires the application to pop notes; sending it never
  completes shutdown. Active-phase room gates still protect their
  downstream writes. Host `Close` and outside-caller terminal semantics
  remain those of skein's host (skein's `forge-setup.md`, 6.3) and
  programming-model.md, 5.2: closed follows settlement, never a request.

### 5.2 The setup host's plans

| Plan | Forge | When | skein's `forge-setup.md` |
|---|---|---|---|
| `Reconcile { forge, adopted }` | Forgejo, managed, owned or fake | each managed, owned or fake start; a managed connect; the box's `started` (5.1) | section 4 |
| `Adopt { forge, repository }` | Forgejo, managed, owned or fake | an adoption, before its reads; the fake's start (8.4) | 4.5 |
| `Create { forge, org, repository, files }` | Forgejo, owned | the box's `started`, when the set up asked for a first repository (6.7) | 4.9 |
| `Check { forge }` | Forgejo, connected | a connect; a replacement | 4.6 |
| `TakeOver { forge, admin_token }` | Forgejo, owned or fake | a start or the box's `started` with a handover, even with a kept `AdminToken` (6.5) | 4.7 |
| `Reachable { forge, installation?, page }` | every kind | the picker | 4.8 |
| `Manifest { forge, target }` | GitHub | Connect, managed | 5.1 |
| `Convert { forge, code, state }` | GitHub | the redirect back | 5.2 |
| `Install { forge }` | GitHub | Install the App, or Install on another account | 5.4 |
| `Accept { forge, installation, state }` | GitHub | the setup route's visit | 5.4 |
| `Reconcile { forge }` | GitHub | after each connect | 5.6 |
| `Paste { forge, app }` | GitHub | Connect with a pasted App; Replace | 5.8 |

- **What each does, and when the routing runs it,** is forge.md's (the
  setup's host); each plan's steps, its bounds and what it refuses are
  skein's.
- **`Reachable`** serves every kind, pages counted from 1, at most
  `reachable` rows, which temper gives from `picker.page`
  (configuration.md, section 7), and ends `Done(Reachable { rows, more
  })`, `more` when the page is full; it writes nothing. On GitHub it
  reads one accepted installation's repositories, `installation` its
  id, on a token minted for the page and dropped; an id not in
  `Accepted` stops it `Installation`, `Missing`. On a connected Forgejo
  it reads what the API token can push to; an `installation` given
  there stops it `Installation`, `Unexpected`. On a managed, owned or
  fake Forgejo it reads every repository the admin can see, by id ascending,
  on the admin's token, which stays the setup host's, and is refused
  `Missing` or `NotAdmin` as reconcile's first step is. Forgetting that
  token makes the forge connected, and its picker with it.
- **Outcomes:** `Done`; `Manifest { state, page }`, the state and the
  manifest page's bytes, which temper holds with the state; `Link`, the
  install link; `Lacking`, done, with each permission or event the App
  lacks named.

### 5.3 Reconcile's given values, and its answers

- **The plan is skein's.** Its six steps, in this order (1 the admin; 2
  the bot, marked `<word>:bot`; 3 the bot's two tokens; 4 the bot's id;
  5 each org, when owned; 6 each adopted repository's access, the bot a
  collaborator and the repository's hook), each read first and written
  only where the forge differs; its wait for the forge; its bounds; and
  what it refuses are held by skein's `forge-setup.md`, section 4. It
  does no io and reads no configuration: it is given values, says each
  request, and the setup host runs it.
- **Its given values, from temper's configuration:**

  | Reconcile's value | temper's |
  |---|---|
  | the forge's number | the forge's `number` (configuration.md, section 3) |
  | the bot's login and email | the setup's `bot.login` and `bot.email` (section 8); at a connect as its admin of a forge connected before, the connected user's login (5.5) |
  | the tokens' names | `temper-api` for the API's, `temper-git` for git's |
  | the orgs, and their owner | none: reconcile makes no org; an owned forge's org is its first repository's, made by `Create` (6.7) |
  | the repositories | the forge's adopted repositories, the root's, restored before the start's reconcile |
  | the name a failure gives the admin | `temper-admin` |
  | the listener's address | `application.webhooks.listener`'s, when configured; none otherwise. Each hook's URL, `http://<address>/hooks/<number>`, is derived from it (skein's `forge-setup.md`, 4.1) |
  | the word | `temper`, so the bot's mark is `temper:bot` |
  | its limits | `orgs` from `setup.orgs`, `repositories` from `forge.client.repositories`, `reachable` from `picker.page`, and `deadline`, `backoff` and `backoff_max` from `setup.deadline_ms`, `setup.backoff_ms` and `setup.backoff_max_ms` (configuration.md, section 7) |

  The API's address is the forge's `api` (configuration.md, section 3).
  temper-forge-setup maps the setup onto these values and runs the
  kit's check on them, at each start and for every command (section
  11); a value the check refuses comes back as its `Place` (skein's
  `forge-setup.md`, 6.1), and is named by the path this table gives for
  it.
- **What the setup host answers, as needs:**

  | Reconcile asks for | The setup host answers |
  |---|---|
  | the admin's token | at a managed connect, the pasted value; otherwise the `AdminToken` record |
  | the API and git tokens, the bot's id | the `ApiToken`, `GitToken` and `User` records, or none when absent |
  | the webhooks' secret | the `WebhookSecret` record, or none, and reconcile then draws one |

  Each answer is a record or a pasted value, never a file: a forge's
  `api_token`, `git_token`, `user` or `webhook_secret` file, from a
  temper before the records, is not read, and reconcile makes what is
  missing afresh (section 3). Each item reconcile writes is kept as the
  forge's record; a kept `WebhookSecret` is never drawn again. The
  fake's start-time adoption (8.4) reads the same records, after its
  handover and reconcile.

### 5.4 Access at adoption

- **`Adopt { forge, repository }`** runs before an adoption's reads on a
  managed, owned or fake Forgejo (skein's `forge-setup.md`, 4.5): the bot made
  a collaborator with write, and the repository's hook made when the
  listener is configured, with the forge's secret. Stopped, the adoption
  is refused, named (`../domain/forge.md`, access at adoption). A
  `403` here, at reconcile's step 6 or at the picker's read, is the
  admin's token without `write:repository` (skein's 4.3, step 1): it
  is shown as that scope missing, named with its step.
- **At each start,** reconcile's step 6 checks each adopted
  repository's access, and repairs what changed.
- **A fake forge,** until this section is built: its repositories are
  its entry's, so the setup host runs `Adopt` for each of them at the
  engine's start, before the forge connects, in place of each
  adoption's (8.4, the fake's start-time adoption).
- **Nothing org-wide:** the bot reaches the repositories adopted, a
  person's own among them, and no other.
- **A connected Forgejo, and a managed one forgotten,** grant nothing at
  adoption: the person makes the bot a collaborator, and the repository
  polls unless the person adds its hook (section 4).

### 5.5 Connecting as its admin, and forgetting

- **"Connect as its admin"** (`Connect` with `ForgejoAdmin`;
  `../client/forge.md`, the deployment's page), on a Forgejo neither owned,
  connected nor connecting, whose `setup` entry is the string `"managed"`
  (section 8): the site admin's token, with the preparation's scopes
  (6.3), pasted, kept by jig until its run ends. One run holds the paste:
  `Reconcile` reads it, never an older `AdminToken` record; its writes
  are kept, then the forge's `Origin` under `AdminOrigin`. With `first`,
  `Create` (6.7) runs with the same paste. Only after both plans are Done
  is the token kept as the forge's `AdminToken` record, and the forge
  connects, managed.
  Without `first`, Create is skipped. This connect never runs `TakeOver`.
  Every end erases the carried value and drops the held token; a stop
  keeps no admin token. Each later start
  reconciles from that record and connects, with no Create or `TakeOver`;
  without the record it offers Connect, not connected. This form serves
  no organizations or organization owner and no listener: it polls.
- **Its bot:** on a forge with no records, the setup's `bot.login`, made
  when missing; on a forge with records, the recorded user's login.
  Reconcile uses it only when its full name carries `temper:bot`, or stops `Unmarked`,
  named, and restores the connection's previous state.
- **A first organization named as the bot** is refused at admission
  (`FirstIsBot`), before any run or write, and its carried value named
  unused: a Forgejo user and organization can't share a name, so Create
  would stop at the organization, and a second paste couldn't help. The
  check compares `first`'s organization with the setup's `bot.login`,
  ASCII case-folded, as Forgejo compares names.
- **A Forgejo with no `setup` entry** refuses it, naming
  `application.setup` (section 16).
- **"Forget the admin's token"** (`Forget`): the `AdminToken` record is
  erased, and the forge is connected, its bot, tokens, id and hooks
  kept; no reconcile runs for it after.

### 5.6 What holds of every plan on Forgejo

- **Plaintext to loopback alone, for owned and fake:** an owned or fake
  forge's `transport` is plaintext, and one whose transport is TLS is
  refused at start, naming it (section 15). A managed forge's is TLS, or
  plaintext to a loopback host alone; plaintext to another host is refused
  at start, naming it.
- **The bot's email** is the setup's `bot.email` when reconcile makes
  the bot, and stays as it is after: a change to `bot.email` reaches
  only a bot made after it, and an existing bot's email is the
  person's to change, on Forgejo. temper reads no mail of the bot's,
  and its commits name the identity's `author` (configuration.md,
  section 3), not this email.
- **The scopes** of the tokens are section 4's.
- **What it cannot set:** Forgejo's settings, which its API does not
  change (skein's `forge-setup.md`, section 4): the webhooks'
  `ALLOWED_HOST_LIST`, and whether Actions are enabled. A managed forge
  whose host list refuses temper's listener delivers no webhook, and
  polling keeps up (section 10).

## 6. Owned: prepare and run

### 6.1 Its inputs and its binaries

- **Inputs:** the Forgejo binary, and the runner's, or no runner, each
  an absolute path or `"fetched"`, temper's pinned build in the cache
  (6.6); the address Forgejo listens on, a socket address on loopback;
  the person's login and email; the runner's labels, capacity and
  `PATH`; the forge's directory.
- **The binaries are checked** by every command that runs them: the
  first line of each one's version equal to temper's pin, Forgejo's and
  the runner's (`skein-forge`'s `VERSION` and `RUNNER_VERSION`, skein's
  `forge.md`, section 4.3), the versions skein's conformance checks
  (`fake-forge.md`, section 7). Another binary refuses the command,
  naming its path and what it printed. A fetched binary is checked so
  too, after its digest at its fetch; temper downloads nothing but by
  `temper forge fetch` (6.6).
- **One pin,** `skein-forge`'s, read by owned mode, the throwaway and
  the conformance alike, so the Forgejo temper runs is the Forgejo whose
  facts it checked (13.3): the versions, and beside them each fetched
  binary's SHA-256 for each architecture temper fetches for (6.6).

### 6.2 Forgejo's settings

`app.ini`, written by every preparation from the setup and `state`,
private, never edited by hand:

- `RUN_USER` the process's user, `WORK_PATH` the work path;
- `[server]`: `HTTP_ADDR` and `HTTP_PORT` from the address, `ROOT_URL`
  `https://<public>/` with `DOMAIN` its host when `public` is given, and
  `http://<address>/` otherwise, `DISABLE_SSH`, `OFFLINE_MODE`;
- `[database]`: SQLite in the work path;
- `[repository]`: its root in the work path, `DEFAULT_BRANCH` `main`;
- `[security]`: `INSTALL_LOCK`, `SECRET_KEY` and `INTERNAL_TOKEN`;
  `[oauth2]`: `JWT_SECRET`; each drawn once, at the first preparation,
  and kept in `state`;
- `[service]`: `DISABLE_REGISTRATION`; `[mailer]`: off;
- when the forge signs its people in through temper, named by
  `application.sign_in.forges` (`sign-in.md`, 9.7): `[oauth2_client]`
  `ENABLE_AUTO_REGISTRATION = true`, `USERNAME = preferred_username` and
  `ACCOUNT_LINKING = login`, and `[service]`
  `ALLOW_ONLY_EXTERNAL_REGISTRATION = true` (`sign-in.md`, 9.4). The
  source itself is not the preparation's: it is added by the command
  `temper forge sign-in` prints, as for a managed Forgejo (6.8);
- `[webhook]`: `ALLOWED_HOST_LIST` the webhooks' listener's IP address,
  when one is configured, and Forgejo's default otherwise, which refuses
  loopback;
- `[actions]`: `ENABLED` when the setup has a runner;
- `[log]`: files in the work path;
- `[api]`: Forgejo's defaults, which the forge component reads at its
  start (skein's `forge-connection.md`, section 3).

### 6.3 The preparation

`temper forge prepare [DIRECTORIES] FORGE`, before Forgejo runs. It takes
Forgejo's lock (6.4), and the runner's too when it writes the runner's
files: at a first preparation, and for a runner older than its pin
(item 3) or added (item 4); checks the binaries; and decides by
`state`:

1. **The first preparation:** the layout of section 3; `app.ini`;
   `forgejo migrate`; temper's admin, `temper-admin`, made as a site
   admin with a password Forgejo draws and nobody keeps, and its token
   made by `admin user generate-access-token` into `admin_token`, the
   handover (section 3), with the four scopes `write:admin`, `read:user`,
   `write:organization` and `write:repository` (skein's `forge-setup.md`,
   4.3, step 1: the users, their tokens and the orgs; `GET /user`; the
   orgs' and their repositories' making and reading; the repositories'
   routes of `Adopt`, reconcile's step 6 and the picker's read), and no
   other, under a name that
   ends in a random suffix, since Forgejo refuses a second token of one
   name; the
   person's account, a site admin with a password Forgejo draws into
   `person_password`, to change at their first sign-in; the runner's
   secret drawn into `runner/secret` (40 hexadecimal digits, no line
   feed), registered as a global runner by `forgejo-cli actions register
   --secret-file`, and the runner's connection written from it into the
   runner's configuration (`server.connections`), its token in a private
   file the connection names (`token_url: file:`); the work path's
   repositories directory, which `forgejo dump` needs. `state` records
   the first preparation's start before anything else, and its end last.
   A first preparation that `state` says began and did not end is cut
   short: the next one removes what it left (the work path, the runner's
   directory, `admin_token`, `person_password`) and begins again from an
   empty layout, since Forgejo refuses a user made twice and never shows
   a drawn password again. A forge's directory that holds Forgejo's files
   with no `state` is refused, naming it: the preparation removes only
   what a first preparation it recorded left.
2. **A later preparation at the same versions, with a runner exactly
   when `state` records one:** `app.ini` written again, so a change of
   the setup reaches Forgejo; `admin_token` made again when missing,
   which it is once the engine has taken it (6.5), under a fresh suffix,
   for temper's admin, which Forgejo's command line cannot make twice:
   an admin a person deleted fails the preparation, naming it; nothing
   else. It takes Forgejo's lock alone, so a box restarts a Forgejo,
   after its preparation, while the runner runs.
3. **An upgrade:** a recorded Forgejo older than the pin is dumped
   (`forgejo dump`) into `backups/forgejo-<old>-to-<new>.zip`, written as
   `backups/dump.partial.zip` (Forgejo adds `.zip` to a name without it)
   and renamed, never overwritten (a dump already there is not made
   again), then migrated, then recorded. A recorded runner older than its
   pin has its files prepared again, under the runner's lock too, with no
   dump, since the runner keeps no data. A recorded version newer than
   its pin, Forgejo's or the runner's, refuses: Forgejo's migrations only
   go forward.
4. **A runner added or removed:** the setup and `state` disagree on
   whether there is one, and `app.ini`'s `[actions]` follows the setup
   (6.2). A runner added is prepared under the runner's lock too, as a
   first preparation prepares it: its secret drawn into `runner/secret`
   when that file is missing, and kept otherwise; registered; its
   connection written. A runner removed is recorded as none, and its
   files and its registration are left, unused, so that a runner added
   again registers as the same one. A runner's recorded version is
   compared with its pin only while the setup has one. Either may come
   with an upgrade, in one preparation.

- **Forgejo's command line only,** on the work path, never its web: under
  the lock no Forgejo runs, so nothing writes its database beside the
  preparation.
- **No secret in an argument.** A password is drawn by Forgejo (the user
  creation's random password) and read from its output; the runner's
  secret goes by file (`--secret-file`). The runner's `create-runner-file`
  takes its secret only as an argument, so the preparation never runs
  it: it writes the runner's connection itself, in the runner's
  configuration, with its token in a file (13.3).
- **Its commands run on skein-io's processes,** as the box's children
  do: each spawned with its arguments and the spec's environment, its
  output read through pipes within a bound, its exit status awaited.
- **Interrupted** by a termination signal while a command runs, it
  kills the command's process group and exits 1, printing `forge <n>:
  interrupted`. `state` is not marked ended, so the next preparation
  begins again from an empty layout, for a first one, or repeats, for a
  later one or an upgrade.
- **Its commands' output,** kept for a failure's message, is masked of
  every secret it holds, as the throwaway Forgejo masks its own.

### 6.4 Running

`temper forge run [DIRECTORIES] FORGE forgejo|runner`:

- **Checks** the binary, and that `state` records a preparation at the
  pinned versions, with a runner exactly when the setup has one;
  refuses otherwise, naming what to run: `temper forge prepare` for a
  forge not prepared, a preparation that did not end, an older version,
  or a runner added or removed (6.3); no command of this temper for a
  newer version, or one it cannot read.
- **Takes the lock,** an exclusive lock, taken without waiting, on a
  file of the forge's directory, Forgejo's or the runner's. Two Forgejos
  never share a work path, and no preparation runs under one: the
  preparation takes Forgejo's lock, and the runner's too when it writes
  the runner's files (6.3), and refuses, naming the forge, while one it
  needs is held.
- **Checks Forgejo's listener,** when the box gave it one (7.1):
  descriptor 3, with `LISTEN_FDS=1`. It checks that the descriptor
  listens at the setup's `listen` (skein's `listener_at`, `shell.md`,
  6.6) before taking the lock, refusing otherwise, naming the descriptor
  and the address it found. Forgejo then takes the socket whose address
  its `app.ini` names, in place of binding it (its `modules/graceful`,
  which reads no `LISTEN_PID`). Given none, as under every other
  arrangement (7.4), Forgejo binds its address itself.
- **Stays Forgejo's parent when given a listener.** Forgejo 17 sends
  its parent `SIGTERM` once when it starts with `LISTEN_FDS` set (its
  `KillParent`, which no setting turns off), so forge run doesn't exec,
  which would make the box that parent. It holds the lock for Forgejo's
  whole life, adopts the listener into skein-io, and spawns `temper
  forge child --parent PID --signal kill -- forgejo web …`, PID its own, with the
  listener at the child's descriptor 3, the spec's environment ending
  in `LISTEN_FDS=1`, and the spec's directory. Forgejo's standard input
  is empty; its output and errors are relayed to forge run's own.
  - **The hidden `forge child`** reads no document and takes no
    directories. It execs its program with what it was given, through
    skein's `exec_dying_with_parent` (skein's `shell.md`, 3) with the
    parent-death signal its caller names, Kill for forge run, so that a
    forge run killed takes Forgejo with it at once. Kill, not Terminate:
    a Forgejo still stopping gracefully when nothing waits for it would
    meet the restarted forge run's on the box's socket and its database
    (QA, `qa_forgejo_dies_when_its_forge_run_parent_is_killed`).
    Forgejo's graceful stop is forge run's forwarded Terminate. A
    failure, the parent changed or the program missing, prints `temper
    forge child: WHY` and exits 1.
  - **Its shutdowns:** one that io names as Forgejo's (skein's `io.md`,
    section 7) is ignored, every time; every other is forwarded to
    Forgejo as the same Terminate or Interrupt. Forgejo's exit ends
    forge run with Forgejo's code, or 128 and the signal's number, so
    the box supervises forge run as it would Forgejo, and restarts it
    when Forgejo is killed. The decisions are a pure machine in
    `temper-forge-setup`; the shell only carries them out.
  - **On Linux only.** On macOS a shutdown names no sender (kqueue's
    `EVFILT_SIGNAL`) and no parent-death signal exists, so the box gives
    Forgejo's step no listener and Forgejo binds its own port, as above.
- **Replaces itself** (`execve`) otherwise: for the runner, and for
  Forgejo given no listener. It becomes `forgejo web` or
  `forgejo-runner daemon`, with the spec's arguments and the spec's
  environment alone: cleared, then `HOME` in the forge's directory,
  `PATH`, and git's configuration isolated, as the throwaway Forgejo's
  is. The lock crosses the exec as the process's standard input, so the
  process holds it for its life, and the process a host supervises is
  then Forgejo's or the runner's own, with its signals and its exit
  status.
- **Outside skein's shell, in one module.** skein's shell spawns but
  never execs, and its locks never pass to a child (skein's
  `programming-model.md`, 2.1; skein's `io.md`, section 5). So the lock and
  the exec are std's, `File::try_lock` and `CommandExt::exec`, with no
  `unsafe`, in one module of `temper-forge-setup-shell`, which the
  preparation's lock uses too, and which builds `forge child`'s command
  for skein's parent-death exec; the preparation's commands and
  Forgejo's parent loop are not in it, and run on skein-io (6.3). It is
  the one place temper locks a file or uses `std::process` outside
  skein: a boundary test scans the production crates' sources (`crates/*/src`, test files aside) and
  fails the gate when either appears outside that module, but
  `std::process::ExitCode` in the binary's `main`. Test crates
  (`testing/`, `tools/`) are not scanned. It moves onto skein when
  skein's shell offers both (section 15).
- **The runner's spec:** its configuration file (its connection,
  capacity, labels, timeouts, its jobs' directory, no cache), its `PATH`
  from the setup. Its jobs run as the user that runs it (section 15).

### 6.5 Then the engine's start, and the person

- **The handover is the engine's,** for an owned or fake forge, at
  `temper serve`'s start after the records' load and the root's restore,
  or at the box's `started FORGE` (7.1). Whenever the handover file is
  present, even with a kept `AdminToken`, the setup host reads
  `admin_token` once by the service's io, as a private file beneath the
  forge's directory, within `records.value_bytes` (configuration.md,
  section 6). It runs `TakeOver { forge, admin_token }` (skein's
  `forge-setup.md`, 4.7): the token checked as a site admin's, kept as
  the forge's `AdminToken` record, and temper's admin's other tokens
  deleted. Only after `TakeOver`'s `Done` and the record kept does the
  host remove the file. It then runs `Reconcile { forge, adopted }` with
  the root's restored repositories (section 5). A fake then runs its
  start-time adoption (8.4), before `Request::Connect` with access built
  from the records (forge.md, 2.1, 2.2). The forge is
  connecting from the start or `started` until its `Connected`; its git
  value comes from its `GitToken` record (forge.md, 2.5).
- **A present handover is always taken over.** With an `AdminToken`
  record and no handover file, the host reconciles from the record. With
  the file present, `TakeOver` replaces the kept token and deletes the
  older tokens; taking the same token again is harmless. No owned or
  fake forge's `api_token`, `git_token`, `user` or `webhook_secret` file
  is written or read.
- **A step that stops blocks that forge's connect.** A `TakeOver`,
  `Reconcile`, `Create` or `Adopt` stop, a handover read that fails, or a removal
  that fails leaves the forge not connected, named by the plan's step or file
  operation, never a secret; at `started`, it is set up failed. A
  `TakeOver` stop leaves the file; a failed removal blocks reconcile.
  The row says not connected after a start; at `started`, set up
  failed with its `why`, the step and a short reason within the wire's
  256 bytes, a file operation naming the file as "forge N's
  admin_token". The operator's note names the plan's step and its
  reason, and for a file operation the file's full path; never the
  file's bytes. The next start or ask takes that handover over again. The
  engine and every other forge run on. "Set up again"
  asks the box, which answers a started group `started` at once (7.1),
  and the engine runs the same sequence again.
- **Until temper-15, managed and connected Forgejo's old members**
  (`api_token`, `user`, `webhook_secret`, and the old managed form)
  still read their files as before (configuration.md, section 6).
  `temper forge reconcile` remains a command for those forges alone.
  A records-only owned or fake entry is refused: "forge N starts from
  its records: `temper serve` takes its handover".
- **Then reconcile runs as managed's** (section 5), and makes no org:
  the set up's org is its first repository's, made by `Create` (6.7).
- **Then the first repository,** at the box's `started`, when the set
  up asked for one (6.7): `Create`, before the forge's connect.
- **A forge not prepared,** with neither a handover nor an `AdminToken`
  record, is not set up: no access and no connect, including an owned
  forge whose setup says `"start": "asked"`; every other forge starts as
  it would. The deployment's page (`../client/forge.md`, 5.4):
  - **Under the box,** offers "Set up a Forgejo here" for an owned forge
    whose setup says `"start": "asked"`. Its card says CI jobs run here
    unisolated, Forgejo stops with `temper box`, and its files are kept
    in temper's data directory. The routing writes `start FORGE`; at
    `started FORGE` the engine takes the handover, reconciles, makes the
    first repository (6.7) and connects; at `failed FORGE WHY` the page names the step and offers
    it again. Until that line carries the reason, the page points to
    `temper box`'s output for it.
  - **Without the box** (7.4), names `temper forge fetch N`, `temper
    forge prepare N`, `temper forge run N forgejo` and `temper forge run
    N runner`, then a restart of `temper serve`, N the forge's number.
    The connected card offers the repository form.
- **The page's actions** are the engine domain's offers, from the same
  function that gives the set up's offer (`../domain/forge.md`, 4.1).
  "Connect as its admin" needs a managed setup entry (section 8).
- **The connected card** says "Connected as LOGIN", with its id on an
  Account line. Access is "Site admin, with temper's own token" for
  owned, "Site admin, with a token you gave" for managed, and "Its bot's
  tokens, which you gave" for connected. Owned's Runs line names Forgejo
  and its runner on this machine, their versions, and CI jobs running
  here as the person. Its First password line names the path to
  `person_password`, never its password, from set up on: the person's
  first sign-in, through temper too, links their account by it
  (`sign-in.md`, 9.4), and temper keeps no record of that sign-in by
  which to drop the line after it. A stopped owned
  forge says "Restart temper box: its preparation mends it". Owned and
  managed offer "New repository", with the set up's organization,
  repository and project fields (section 15).
- **The person** signs in on Forgejo's web as a site admin with
  `person_password`, and changes it, since Forgejo's API refuses them
  until they do (13.3); makes repositories in their orgs, with Actions,
  and adopts them through temper's client. temper's admin is temper's; a
  person who deletes it makes the next start's reconcile and the next
  preparation stop, naming it.
- **The forge's members** are connected's, written out: `api` the
  address, `transport` plaintext, `git` `http://<address>` (section 8).

### 6.6 Fetching

`temper forge fetch [DIRECTORIES] FORGE`, before an owned forge's
preparation, for a forge whose `forgejo` or `runner.binary` is
`"fetched"` (section 8): a once step of the box (7.1), and a command of
its own for any other host.

- **What it fetches,** each binary its setup gives as `"fetched"`, at
  temper's pins (6.1), for the machine's architecture, `ARCH`:

  | Binary | From | Into |
  |---|---|---|
  | Forgejo | `https://codeberg.org/forgejo/forgejo/releases/download/vVERSION/forgejo-VERSION-linux-ARCH` | `<cache>/forgejo/VERSION/forgejo` |
  | the runner | `https://code.forgejo.org/forgejo/runner/releases/download/vRUNNER_VERSION/forgejo-runner-RUNNER_VERSION-linux-ARCH` | `<cache>/forgejo-runner/RUNNER_VERSION/forgejo-runner` |

  `ARCH` is `amd64` or `arm64`; another machine refuses, naming its
  architecture, and its binaries are given by path.
- **One request, one answer.** Each URL answers `200` with the binary,
  never a redirect: skein-http follows none, and any other status
  refuses, naming the URL and the status.
- **A mirror,** the owned entry's `mirror`, absent by default (section
  8): an `https` origin for each binary, in place of codeberg.org's and
  code.forgejo.org's, its paths and digests the pins' still. A host that
  keeps its own copies names it; the process tier needs none, its
  binaries placed where the fetch looks (13.4).
- **TLS with the machine's roots,** read at the command's start as
  skein's shell reads them (skein's `shell.md`, 6.2): the bundle
  `SSL_CERT_FILE` names, or the first of the distributions' bundles that
  exists. skein-tls embeds none, so a machine with neither refuses the
  command.
- **The digests are pinned,** one SHA-256 per binary and architecture,
  in `skein-forge` beside `VERSION` and `RUNNER_VERSION` (skein's
  `forge.md`, 4.3), and required: a fetched binary is the build the
  conformance checked, byte for byte.
- **Streamed, never held:** the body's chunks are asked for one at a
  time, as the file takes them; each is hashed into the digest, which
  is incremental, and written at its offset into `<file>.part`, private,
  within `setup.fetch_bytes` (configuration.md, section 7). At the
  body's end, a digest equal to the pin makes the part file `0o755` and
  renames it over its name.
- **A wrong digest** removes the part file and refuses, naming the URL,
  the pinned digest and the digest it got. Nothing is left under the
  binary's name.
- **A file already there is hashed,** never fetched again: its pin's
  digest is nothing to do; another is refused, naming its path, and
  never overwritten, so a binary a person put there stays theirs to
  remove.
- **A part file an interrupted fetch left** is written again from its
  start.
- **Then the preparation checks its version** as every command that
  runs it does (6.1); a digest is no version.
- **Its code:** the fetch's machine in `temper-forge-setup`, its
  requests, chunks, writes and digests its inputs and outputs, and its
  host in `temper-forge-setup-shell`, on skein-io, skein-http and
  skein-tls (section 11). It holds no credential and writes no record
  (section 9).

### 6.7 The first repository

- **Asked with the set up.** `SetUp` carries an organization (default
  `work`), a repository (default `hello`) and a project: a new
  project's name, the repository's until the person changes it, or an
  existing project's number, which the first run never offers
  (`../draft/first-run.md`, 2.4). A new one is refused "a new project"
  until jig makes projects at runtime (jig's `MakeProject`, its
  `domain/core.md`, 6.1). The forge domain keeps
  them with the forge, the set up's intent: one per forge, in memory
  alone, replaced by the next `SetUp` it admits, and kept until the
  repository is made or the set up has failed.
- **Asked at a managed connect.** `ForgejoAdmin.first` carries the same
  organization, repository and project, and the person's runner label
  (default `docker` in the client and ctl). After Reconcile and its
  `AdminOrigin` write are kept, Create makes the repository
  before the admin token is kept and the forge connects (5.5). A missing
  organization is owned by the admin Create has verified. No listener is
  supplied, so Create adds no hook.
  The forge's row carries `admin`, the string form's admin-connect offer;
  `first.label`, the runner label; and `ended_stop`, an optional
  `AdminStop`: `NotAdmin`, `Refused { status, stage: SetupStage }`,
  `Unmarked` or `Other`. `SetupStage` names admin, bot, token, org,
  repository, file, app, installation or state. `ended` is the separate
  diagnostic; the client chooses its stop's words from `ended_stop`.
- **Made after reconcile.** At the box's `started FORGE`, once
  `TakeOver` and `Reconcile` are done (6.5), the setup host runs
  `Create { forge, org, repository, files }` (skein's `forge-setup.md`,
  4.9) by the admin's token: the org, private and owned by the person;
  the repository, private, with Forgejo's README on `main`; the bot a
  collaborator with `write`, and its hook on the webhooks' listener;
  then the starter workflow, below. Then the forge connects, as before
  (6.5). Each step finds what is already there and leaves it, so a set
  up asked again finishes what the last one left. The engine's limits
  for it are 1 file of at most 4 KiB (skein's `forge-setup.md`, 4.1).
- **The starter workflow,** `.forgejo/workflows/ci.yml`, its commit
  the bot's, run on a push to `main` and on each pull, on the owned
  runner's label, `temper-host`. At a managed connect it uses `first.label`
  instead: 1–64 ASCII letters, digits, `.`, `_` or `-`, checked at the wire
  and again before the YAML is made. The workflow is otherwise the same:

  ```
  on:
    push:
      branches: [main]
    pull_request:
  jobs:
    check:
      runs-on: temper-host
      steps:
        - run: echo "temper's starter check passed; replace this step with your own checks"
  ```

  Its push is the repository's first CI run, on the owned runner: the
  owned forge enables Actions (6.2), and Forgejo gives a repository
  made through its API the Actions unit (skein's `forge-setup.md`,
  section 8). The step needs no network and no action from elsewhere.
- **The row says it,** the forge's first repository beside its
  connection (`../client/forge.md`, 5.4; `../client/README.md`, 4.6):
  none; making `ORG/NAME` from the `SetUp`'s admission; made
  `ORG/NAME`, with its project, once `Create` is done; or failed
  `ORG/NAME`, at the step that stopped (the admin, the org, the
  repository with its access, or the file). A `Create` that stops
  blocks the connect as any step does (6.5): the forge is set up
  failed, its `why` naming the step, and "Set up again" asks with the
  same fields. A restart drops the row's first repository to none.
- **Adopted by the client, for now.** Once the row says made, the
  client sends, under the same party, jig's `MakeProject` for a new
  project, its name and policy 1, the default document's (8.3), then
  `Adopt`: the project, the forge,
  `ORG`, `NAME`, landing on `main`, owned, the project's home, with CI.
  `temper ctl forge-setup` does it (`../client/tools.md`, 5.5), and so
  does the set up flow's Ready step (`../client/forge.md`, 5.4). It is
  transitional, until jig carries a deployment owner's act after
  `SetUp`'s admission (section 15).
- **A restart before the repository is made** loses the intent: the
  start's sequence makes no repository, and the forge, set up by its
  record, no longer offers "Set up a Forgejo here". The person makes it
  with "New repository" (section 15).
- **Its code:** the intent in `temper-engine-domain-forge`; `Create`'s
  start and its end in `temper-service`'s setup; the starter workflow's
  bytes and the plan's limits in `temper-forge-setup`.

### 6.8 Signing in through temper

`temper forge sign-in [DIRECTORIES] FORGE`, for a Forgejo of
`application.sign_in.forges` (`sign-in.md`, section 9), managed or
owned, on temper's host, before `temper serve`:

1. **The provider's key,** when its file, `engine.clients.provider.key`,
   is missing: `openssl genpkey -algorithm RSA -pkeyopt
   rsa_keygen_bits:3072`, run with umask 077 into a file beside the
   key's and renamed over it, so the key is PKCS#8 PEM, mode 0600 and
   the service's own user's. It is never printed, copied or logged; a
   key there is left as it is. No `openssl` on the `PATH` refuses the
   command, naming it.
2. **The forge's client secret,** when its file, the forge's client's
   `secret`, is missing: 32 bytes of the system's randomness in
   base64url, written whole and private (section 3). A secret there is
   left as it is.
3. **It prints,** never running them: the `add-oauth` command
   (`sign-in.md`, 9.5) with the client's id, the secret file's absolute
   path in `--secret "$(cat PATH)"`, never the secret's bytes, and the
   discovery document's URL at the public origin, or the listener's with
   none; for an owned Forgejo, with the binary and its `--work-path` and
   `--config`; and Forgejo's settings of `sign-in.md`, 9.4, which an owned
   Forgejo's `app.ini` already holds (6.2), and a managed one's owner
   writes.

Then `temper serve`, and the printed command, once, on Forgejo's host.

- **Printed for an owned Forgejo too, not run** (with o/sign-in,
  2026-10-23), for two reasons: `add-oauth` fetches temper's discovery
  document at once and adds no source when temper does not answer, and
  the preparation runs before `temper serve` (6.3, 7.2); and `add-oauth`
  takes its client secret only as an argument, where the preparation
  puts no secret (6.3). A source added by Forgejo's command line while
  Forgejo runs works at once, so the order costs a person nothing.
- **Its code:** the command, and the key's and the secret's making, in
  `temper`'s `forge_sign_in.rs`, dispatched from `forge.rs`; the printed
  bytes in `temper-forge-setup`'s `sign_in.rs`.

## 7. The box: wiring only

### 7.1 What it runs

`temper box [DIRECTORIES] [--echo none|steps]` runs the configuration's steps, which
`temper-fake`'s `steps` gives (8.4), each a command of the `temper`
binary, as a child process given the box's three directories as flags,
`--config C --data D --cache K`, each absolute (configuration.md,
section 12), written `DIRS` below:

```
temper box
├── temper fake llm DIRS              long, with the demo (8.4)
├── temper forge prepare DIRS 1       once, per fake forge: its handover
├── temper fake forge DIRS 1          long, per fake forge (8.4)
│                                     the demo's forge's two and the fake LLM, the demo's group: asked (below)
├── temper forge fetch DIRS 3         once, per owned forge whose binaries are fetched (6.6)
├── temper forge prepare DIRS 3       once, per owned forge
├── temper forge run DIRS 3 forgejo    long: Forgejo's parent on Linux with fd 3 (6.4)
│   └── forgejo web                   via temper forge child; execs without fd 3
├── forgejo-runner daemon             long: temper forge run DIRS 3 runner
│                                     these, the forge's group: here, or asked (below)
└── temper serve DIRS --box           long: its own tree (README.md, section 4); its setup
│                                     host takes each handover and reconciles (section 5);
│                                     its listeners from fd 3, then its pipe to the box (below)
```

- **An empty configuration directory,** absent or with no entry, is
  written first by `temper init` (8.2), in the box's own process, before
  the document is read; the box says so on its terminal, `First run:
  wrote the configuration, <path>` (7.2), and goes on to its steps.
  Otherwise the box
  never writes the document: a configuration directory with entries but
  no document is refused as every command refuses it.
- **An owned forge's group,** its fetch, its preparation, its Forgejo
  and its runner, runs in its place in order at every start of the box
  when its setup has no `start`. With `"start": "asked"` (section 8) the
  box knows the group's steps and starts none of them until `temper
  serve` asks on the pipe, or until the box's record
  `<data>/box/started/FORGE` exists: one empty file per forge, written
  once the group's preparation has exited 0, so that a later box runs
  the group in its place, as an unasked one, before `temper serve`. The
  record is written beneath the data directory, `box` and `box/started`
  made as needed, mode 0600. When it cannot be written, the box's stderr
  names why, the group answers `failed FORGE record not written` and is
  dormant again. At its start the box looks for each asked group's
  record; one it cannot look at is named on its stderr, and its group
  stays dormant.
- **The demo's group,** the fake LLM, then the demo forge's preparation
  and its fake forge (`application.demo.forge`, 8.4), is always asked,
  and writes no record: the demo's state is the engine's alone (8.4),
  so a box started again runs it only when `temper serve` asks, which
  serve does at the person's choice and at each of its starts while the
  demo runs. Its `started` follows its long steps' start, as the steps'
  order takes them (7.2). At the demo's end serve sends `stop FORGE`,
  and the box stops the group (below, 7.2): a box started after the end
  runs none of it, since serve never asks again (8.4). A fake forge
  other than the demo's runs in its place, unasked.
- **The listeners are the box's,** every step's, from the box's start
  to its end, a dormant group's among them: so that the port a step
  reports is its own from before it starts, and a second first run's
  init finds the first one's ports taken (8.2). Each step names its
  listeners, the socket addresses the document gives it (7.3): `temper
  serve` the clients' and the webhooks' listeners, a fake its `listen`,
  on Linux, `temper forge run N forgejo` the owned forge's `listen`
  (6.4). On a
  first run the box keeps the sockets `temper init` chose (8.2); on a later
  start it opens each at the document's address before any step, with
  skein's `open_listener`, and a port taken refuses its start, naming
  the port and the document, as any start is refused. It holds each
  one, never accepting, and gives it to its step at every spawn, a
  restart's among them, as the descriptors 3, 4, … in the step's order,
  with `LISTEN_FDS` set to their count in the step's environment and
  dropped from its own: systemd's convention, which skein's shell reads
  (skein's `shell.md`, 6.6) and Forgejo reads as well (6.4). The step's
  own `Listen` for that address then takes the socket it was given
  (skein's `io.md`, 3.2), so nothing above io knows it was given one. A
  step with no listener is given none, and no `LISTEN_FDS`. On Linux,
  `temper forge run N forgejo` is given the owned forge's `listen` and
  stays Forgejo's parent (6.4); on macOS it is given none, and a fresh
  box closes the reservation `temper init` made for its port, so
  Forgejo binds it itself. A listener at port 0 is not held: its step binds it, as without the box, since a
  port the kernel chooses can't collide. A held one could not be claimed
  either, since the step's `Listen` names port 0, not the port chosen.
  It changes at each restart, as it does without the box. `LISTEN_FDS`
  counts only the sockets given.
- **The pipe,** two of them, made again at each spawn of `temper
  serve`, at the two descriptors after its listeners: 3 + *n*, serve to
  box, and 4 + *n*, box to serve, *n* its listeners' count, so fds 5
  and 6 under the default document's two. `--box` tells `temper serve`
  to adopt them, and a serve given `--box` whose descriptor 3 + *n* or
  4 + *n* is not an open pipe refuses its start, naming the fd, *n*
  being the `LISTEN_FDS` it was given, 0 without it; without `--box`, as
  under every other arrangement (7.4), serve has no pipe and leaves
  those descriptors alone. One line each, ending in a line
  feed, `FORGE` the forge's number:

  | Line | From | Says |
  |---|---|---|
  | `start FORGE` | serve, at the client's ask (6.5); the demo's, at the playground's start and at serve's start while the demo runs (8.4) | start the forge's group |
  | `step FORGE STEP` | the box | the group's step `fetch`, `prepare` or `run` is starting, for the client's row (`../client/forge.md`, 5.4) |
  | `started FORGE` | the box | the group's long steps are started, Forgejo answers, and its record is written, the demo's group writing none; the engine then takes the handover and reconciles (6.5) |
  | `failed FORGE WHY` | the box | the group stopped: `WHY` names the step by its name, as its log file and the box's terminal name it (7.2), such as `prepare 3 exited 1`: `<step> exited <code>`, `<step> killed by signal <n>`, `<step> did not start`, or `record not written`, cut to fit the line's bound, and holds no secret; a step's own lines are in its log, the last of them under the terminal's `failed` line (7.2) |
  | `stop FORGE` | serve, the demo's, at the demo's end (8.4) | stop the forge's group |
  | `stopped FORGE` | the box | the group's steps are closed, as the box's own stop closes them (7.2), and the group is dormant again; a record it wrote stays |

  One ask at a time: a `start` for another forge while a group starts,
  or while a failed group's steps close, is answered `failed FORGE busy`;
  one for the forge whose group is starting, or whose failed steps
  close, by that group's own lines, its `failed` once they have closed;
  one for a forge with no group, `failed FORGE not asked`; one for a
  started group, `started FORGE`; one while the box stops,
  `failed FORGE stopping`. A `stop` is an ask as a `start` is, one at a
  time: for a started group, the box closes its steps, then answers
  `stopped FORGE`; for a dormant one, `stopped FORGE` at once; for a
  group starting, or whose failed steps close, `failed FORGE busy`; for
  a forge with no group, `failed FORGE not asked`; and while the box
  stops, `failed FORGE stopping`. While a group's steps close at its
  `stop`, a `start` or a `stop` for any forge is answered `failed FORGE
  busy`. An
  answer to a `temper serve` that has exited is dropped: its restart
  finds the handover at its start (6.5). Each line is at most
  `box.line_bytes` (configuration.md, section 7), which both sides
  read within; another line, or a longer one, is named on its reader's
  stderr and ignored. serve's side, its asks, its reads and the states
  they tell the engine's root, is the routing's box link (forge.md,
  2.8).
- **A group's failure** before `started` answers `failed`, and the group
  is dormant again, to be asked again; the box runs on. Once started,
  its long steps are restarted, and given up on, as 7.2 says.
- **The default document's steps:** `temper serve` alone runs at the
  start; the demo's group is dormant until the person chooses the
  playground (8.4), and forge 3's until the client asks for it (8.3).
  No step is a link: serve's first line is the person's.

### 7.2 What it does

- **In order:** each step starts once the one before it has started, when
  that one is long, or has exited 0, when it is once or link. A link
  step starts once the long step before it has written its first line
  to its stdout.
- **A link step** is given that line whole on its stdin, which is then
  closed; it is the link step's alone, and the box's stdout shows the
  long step's lines after it. A long step that exits before writing a
  line hands its restart's first line to the link step. A link step
  runs once per box: the long step's restarts never run it again.
- **Restarts** a long step that exits, after a backoff doubling from
  `box.backoff_ms` to `box.backoff_max_ms` (configuration.md, section 7),
  first running again the steps that are once just before it: a Forgejo is
  restarted after its preparation, so a change of the setup reaches its
  `app.ini`. `temper serve` restarts on its own, with a pipe made afresh,
  and its start takes a handover the preparation left and reconciles each
  managed and owned forge. A token revoked under a running engine stops
  that forge alone, named, and not the engine (forge.md, a forge stopped);
  the engine's next start makes it again. A fake forge's restart is such a
  revoke: it serves its seed afresh, without the bot, so that forge stops
  alone until the engine's next start takes the handover its preparation
  drew again (8.4).
- **Gives up,** exit 1, naming the step, when a once or link step fails,
  but a dormant group's before its `started`, which answers `failed`
  (7.1), or a long one exits more than `box.restarts` times within
  `box.window_ms`. The backoff is counted from a step's exits within the
  window, so it starts again from `box.backoff_ms` once a step has run a
  window without exiting.
- **Stops** at a first termination signal: the long steps in reverse
  order, each by a signal, `box.grace_ms`, a second signal, which turns
  `temper serve`'s drain into an abort (README.md, section 4),
  `box.grace_ms` again, then a close. The machine knows a step only as
  once, long or link, so every long step, and a link step still
  running, is sent both; a dormant group's steps run nothing, and are
  sent nothing. A second termination
  signal closes them all. It exits 0 after a stop it was asked for.
- **Stops a group** at `stop FORGE` (7.1): the group's running steps
  alone, as the box's stop does, in reverse order, each by a signal,
  `box.grace_ms`, a second signal, `box.grace_ms` again, then a close;
  a step that exits during it is not restarted, and a once step still
  running is closed with them. Once they are all closed the box answers
  `stopped FORGE`, the group dormant, says `<label>: stopped` on its
  terminal (below), and runs on; a termination signal during it
  stops the box as above, which closes them too.
- **Its stdout** carries `temper serve`'s stdout alone, the sign-in
  links, but for its first line when a link step follows it, which is
  that step's (8.4). The box reads every step's output a line at a
  time, and writes a line only once it holds it whole, to its line feed
  or its step's end, so that nothing written to its stderr lands inside
  a link when both streams are one terminal.
- **Its terminal,** its stderr, is quiet: the box's own lines alone.
  Before the first sign-in block (below), `First run: wrote the
  configuration, <path>` when `temper init` ran (7.1), `Logs:
  <data>/box/logs/`, and `Starting <step>` as each step of `always`
  starts. After it, each line begins with the time of day in UTC,
  `HH:MM:SSZ `, since the box reads no time zone:
  - for an asked group, named by its label (7.3): `<label>: starting`
    at its `start`, then `<label>: started`, `<label>: failed, <WHY>` and
    `<label>: stopped` as the pipe says them; its `step` lines are the
    client's row's alone;
  - a long step's exit, `<WHY>; restarting in <n> s`, and the box's give
    up, `<WHY>; giving up`, `<WHY>` in `failed`'s words (7.1);
  - `Stopping` at a first termination signal, and `Stopped` once every
    step is closed;
  - and the box's own refusals and failures, as this section and 7.1
    name them: a record, a pipe's line, a log.

  `--echo steps` (`TEMPER_ECHO`; the default, `none`; skein's
  `shell.md`, 6.5) also writes every line each step writes, but serve's
  stdout, prefixed `<step>: `, as it reaches the step's log.
- **The sign-in block:** at `temper serve`'s first link, the box
  writes the lines below, each once the one before it is written, so
  that a terminal shows them in order: the link alone on stdout, the
  rest on stderr.

  ```

  Open this link in a browser on this machine. It signs you in as this
  temper's owner, once and for a short while, so keep it private.
  Opened again once spent, it prints a fresh one here.

  http://127.0.0.1:8367/jig/v1/sign-in/link?secret=...

  Ctrl-C stops temper.

  ```

  Each later link, a fresh one after a spent or expired one was opened
  (jig's `protocol/client.md`, section 8) or a restarted serve's first,
  comes after `<time> A fresh sign-in link:` on stderr. A line given to
  a link step has no block.
- **Its logs,** `<data>/box/logs/`, mode 0700, made as needed: one file
  per step, its name with each space a `-` and `.log` after it
  (`serve.log`, `prepare-3.log`), mode 0600, holding every line the
  step writes to its stdout or its stderr, each after the time the box
  read it, `2026-10-23T14:03:20Z `, but serve's stdout: no file holds a
  sign-in link. At its start the box moves the logs a box before it
  left to `<data>/box/logs.previous/`, replacing that one, so the last
  two runs are kept. A file past `box.log_bytes` (configuration.md,
  section 7) moves to `<name>.log.1`, replacing that one, and starts
  afresh, so a step's logs stay within twice the bound. A log the box
  cannot write is named once on its terminal, and that step's lines
  then go to the terminal as `--echo steps` writes them. Forgejo also
  logs to its own files.
- **A failure's tail:** under each terminal line of a step's failure,
  a long step's exit, a group's `failed` or a give up, the box writes
  the last `box.tail_lines` lines it logged for that step, held in
  memory, each indented four spaces and without its time, then `Log:
  <path>`, indented the same. With `--echo steps` it writes no tail,
  the lines already shown.
- **Its environment:** each step gets the box's; `temper forge run`
  clears it for Forgejo (6.4).
- **Its children** are skein-io's (skein's `io.md`, section 6), each in a
  process group of its own; closing one kills its group, so nothing the
  box started outlives its stop.

### 7.3 What it names

- **Steps only:** each step's name, its arguments and whether it is
  once, long or link; and which of them are a forge's dormant group,
  by the forge's number alone. Its policy is a small machine of its own
  (steps, exits, timers and the pipe's lines in; spawns, signals,
  closes, the pipe's lines and the groups' records out), hosted on
  skein-io.
- **The steps in groups:** `Steps { always, asked }`, `always` the steps
  run in order, and `asked` a list of `Group { forge, before, steps, recorded, label }`,
  each dormant until asked (7.1), `recorded` whether its `started`
  writes the box's record: an owned forge's does, the demo's does not.
  `label` is the words the terminal names it by (7.2), bytes the box
  does not read: `Playground` for the demo's, `Forge N (Forgejo)` for
  an owned forge's. `before` is its place in the order:
  the index in `always` it runs just before (its length puts it last).
  Each step carries its `Stage`, whose word its `step` lines carry, and
  its listeners, the socket addresses the box holds and gives it (7.1). The
  machine's step tests drive a group from dormant to `started`, and to
  `failed`, and a started one to `stopped`, by lines and exits alone
  (13.1).
- **The binary hands it the steps.** `temper box` resolves its three
  directories, runs `temper init` when the configuration directory is
  empty (7.1), checks the document as section 8 says, asks `temper-fake`
  for its steps, `temper-forge-setup`'s with the fakes' before them
  and the demo's group among the asked (8.4), and gives them to
  `temper-box`, each asked group apart.
- **Checked:** a boundary test fails the gate when `temper-box` depends
  on a crate named `temper-*` but `temper-forge-setup`, or names an item
  of it but `steps`, `Steps`, `Group`, `Stage`, `Step` and its `Kind`.

### 7.4 Other arrangements of the same commands

Each needs nothing below the commands to change. None has the box's
pipe: `temper serve` runs without `--box`, an owned forge is set up by
running its commands, and the client names them (6.5).

- **systemd,** a unit per long step: `fetch` and `prepare` as
  `ExecStartPre=` of Forgejo's unit, `run` and `temper serve` as
  `ExecStart=`, the order as the units' dependencies; `temper serve`'s
  unit keeps systemd's default kill of its control group, which ends a
  crashed service's whole tree before its restart (skein's `shell.md`,
  13);
- **the process tier** (section 13.4), which runs the commands itself;
- **QA's environments** (`../draft/qa.md`, 4.1), whose "temper in a box" is
  an owned setup with a scripted LLM, made by its own host from the same
  commands, its LLM key added from the client once signed in
  (`../client/tools.md`, section 10);
- **the QA loop's cluster** (`../draft/qa-loop.md`, sections 4 and 11): the same
  `temper fake llm` and `temper fake forge` programs, with the loop's
  additions (deliveries, git and TLS with skein's test root; 8.4), beside
  `temper serve`, and the demo's start through `temper ctl`;
- **a container image** whose entry point is `temper box`, every process
  of its tree ending with the container.

## 8. The configuration

- **One document,** `temper serve`'s, `temper.json` in the
  configuration directory (configuration.md, sections 2 and 12). Every
  `temper forge` command and `temper box` read it whole, check it
  as configuration.md, section 8 says, `setup` among it, and act on
  their part: they read no file but their own part's.
- **`application.setup`,** a list, at most one entry per forge:

  ```
  "setup": [
    {"forge": 1,
     "bot": {"login": "temper", "email": "temper@example.invalid"},
     "mode": "managed"},
    {"forge": 2, "directory": "forge-2",
     "bot": {"login": "temper", "email": "temper@example.invalid"},
     "mode": {"owned": {
       "forgejo": "/opt/forgejo/forgejo",
       "runner": {"binary": "/opt/forgejo/forgejo-runner",
                  "labels": ["temper-host:host"], "capacity": 1,
                  "path": "/usr/local/bin:/usr/bin:/bin"},
       "listen": {"socket": "127.0.0.1:3000"},
       "person": {"login": "ada", "email": "ada@example.invalid"}}}},
    {"forge": 3, "directory": "demo/forge",
     "bot": {"login": "temper", "email": "temper@example.invalid"},
     "mode": {"fake": {
       "listen": {"socket": "127.0.0.1:8369"},
       "git": "demo/bin/git",
       "owner": "playground",
       "repositories": [{"name": "widget", "landing": "main"}]}}}
  ]
  ```

  | Member | Type | Bounds |
  |---|---|---|
  | `forge` | u16 | the `number` of one entry of `forges`, of kind Forgejo |
  | `directory` | a data path (configuration.md, section 12) | when owned or fake, and only then: a private directory (section 3), unique among `setup` |
  | `bot` | `{"login", "email"}` | a Forgejo login; an email, the bot's when reconcile makes it (5.6) |
  | `mode` | `"managed"`, `{"owned": ...}` or `{"fake": ...}` | managed: temper may be given a site admin's token for the forge from the client (5.5); fake: the demo's (8.4) |
  | `start` | absent, or `"asked"` | when owned, and only then: absent, the box runs the forge's steps at every start; `"asked"`, they wait, dormant, until the client asks for them or the box's record says they ran (7.1) |
  | `owned.forgejo`, `owned.runner.binary` | cache paths (configuration.md, section 12), or `"fetched"` | checked against the pins of 6.1; `"fetched"` names temper's pinned build in the cache, which `temper forge fetch` writes (6.6) |
  | `owned.mirror` | absent, or `{"forgejo", "runner"}`, each an origin | each `https://HOST` or `https://HOST:PORT`, no path: where `temper forge fetch` asks for the pinned paths (6.6) |
  | `owned.public` | absent, or an origin | `https://HOST` or `https://HOST:PORT`, no path: the address people reach this Forgejo at, behind a proxy; written as `ROOT_URL` and `DOMAIN` (6.2) |
  | `owned.runner` | the runner's members, or null for none | `capacity` at least 1; `labels` at least one, at most 8 |
  | `owned.listen` | an address, jig's format | a socket address on loopback; unique among the deployment's listeners and owned and fake forges |
  | `owned.person` | `{"login", "email"}` | a Forgejo login other than the bot's and `temper-admin` |
  | `fake.listen` | an address, jig's format | as `owned.listen` |
  | `fake.git` | a data path | the demo's git, a link to `temper fake git` (8.2, 8.4): the program the engine names in each of the forge's repositories' records, for the worker to spawn in place of `workers[].git` (channel.md, section 4) |
  | `fake.owner` | a Forgejo login | other than the bot's and `temper-admin`: the user who owns the fake's repositories |
  | `fake.repositories` | a list of `{"name", "landing"}` | at least 1, at most 8, each `name` a Forgejo repository name, unique ignoring ASCII case as Forgejo's are, and `landing` a branch name; their contents are `temper-fake`'s (8.4) |

  `"mode": "managed"` takes no `directory`, `start` or `orgs`, and its
  forge in `forges[]` names no token files: its records are made when the
  person connects as its admin (5.5). Its admin token name is `admin`,
  not the owned forge's `temper-admin`. The object form
  `{"managed": {"admin_token": PATH}}` stays until `temper forge reconcile` goes (`../draft/github.md`, 16.6).

  An owned entry names no org: its org is its first repository's,
  made by `Create` (6.7). An owned entry that still gives
  `person.orgs`, or the entry's own older `orgs`, is refused as an
  unknown member, naming its path. A legacy managed entry keeps its `orgs`,
  at least one and at most `setup.orgs`, and a fake entry has none,
  until the entry's form is brought to this table (temper-15).

- **A Forgejo with no `setup` entry** is connected's alone; GitHub takes
  no entry (section 17).
- **The page's actions are the engine domain's offers**
  (`../domain/forge.md`, 4.1), including "Connect as its admin": a
  Forgejo elsewhere needs a managed setup entry with its forge number
  and bot, as above, for that action (5.5). An address alone offers
  no admin connect. The client cannot infer these offers from the
  configuration.
- **The owned card's configuration** gives its data directory, Forgejo's
  and its runner's versions and the person CI jobs run as; the card says
  CI is unisolated and Forgejo stops with `temper box` (6.5;
  `../client/forge.md`, 5.4). After connect it offers "New repository",
  with the set up's organization, repository and project fields (section
  15), including after the without-box commands and restart. The card
  names the `person_password` path, never its value.
- **GitHub's connected card** shows the setup URL to set on GitHub
  (17.3); before any accepted installation, the action is "Install the
  App", and after one, "Install on another account" (17.4).
- **The forge's own members name no credential:** its records hold
  them (section 3). They stay connected's, written out, never derived,
  and checked against its setup; a mismatch refuses every command,
  naming the path: when owned or fake, its `api` is `listen`, its
  `transport` plaintext and its `git` `http://<listen>`, which a fake
  forge does not serve yet (8.4); an identity's `login`, when given, is
  the bot's.
- **A forge's `number` is its stable id** (configuration.md, section
  3), never reused: its records are kept under it, so a forge given
  another number is a new forge, not connected; and records made for
  another kind or address leave a forge not connected, named, with
  nothing erased (section 3).
- **`webhooks`,** `application.webhooks`, `{"listener": address}`, on
  loopback; each Forgejo forge's secret is its `WebhookSecret` record
  (configuration.md, section 3; skein's `forge.md`, section 7).
- **The clients' listener at a fixed address** when a GitHub forge is
  configured: GitHub's redirect and setup URLs name it, so its port is
  never 0, checked at start (17.1).
- **`temper serve` checks `setup`,** and reads one file of it, an owned
  or fake forge's handover (section 3). Under the box, with `--box`, it
  adopts the box's pipe, over which it asks for an owned forge's steps
  whose `start` is `"asked"` (7.1).
- **`application.demo`,** the demo's fake LLM, forge and project, or
  null for none (8.4).
- **An owned example** ships for a person, a test or QA's host to start
  from (8.1).
- **`temper init`** writes a deployment that starts (8.2), with the
  default document (8.3).

### 8.1 The owned example

`docs/examples/owned/` is a configuration directory (configuration.md,
section 12). Its document, `temper.json`, is a whole one, `engine`,
`workers` and `application`, for one owned forge with a runner,
webhooks, one git identity, one worker and two LLM providers, with no
LLM account: a person adds one from the client once the deployment has
started (credentials.md, section 3). Beside it are the files it names
that hold no secret: the agent's configuration, `agent.json`, and the
charters, `charters/`, short texts a person edits. The files that hold
a secret, the worker's secret and its digest, are data paths and never
shipped: `docs/guide/box.md` says how each is made in the data
directory. A person, a test or QA's host starts from a copy, edits the
agent's absolute paths to the copy's, and `temper box --config COPY`
runs it (section 7).

- **Its forge,** forge 1, is owned, in `forge-1`, with a runner whose
  label is `temper-host:host`, the bot `temper`, and the person `ada`,
  whose org is `acme`. Its members are its setup's, written out (above):
  `api` and `git` from `listen`, plaintext; no credential, since its
  records are made at the engine's first start, from the handover
  (6.5); the identity's `login` the bot's.
- **Its landing rule** covers `acme/widget`'s `main`, and asks for CI,
  Actions on the owned runner, the head up to date with its base, and an
  agent's review, gate 1, at a clean head (configuration.md, section 3).
  `acme/widget` is a repository the person makes in the org on
  Forgejo's web, with Actions, then adopts (6.5).
- **Its owner may do the forge's work in the org,** and no more: the
  rules' ceiling, the project's ceiling, its role's authority and its
  projections each hold one grant on the org, `acme`, for each kind a
  change and its goal's issue use (`../domain/forge.md`, section 6):
  read, push, open a pull request, merge, comment, create an issue, edit
  a pull request, update it from its base, close or reopen it, and edit,
  close or reopen an issue; 10, as the process tier's owner holds them.
- **Its LLM providers** (configuration.md, section 2), written out
  whole: 1, OpenAI, its public Responses endpoint and gpt-6-luna with
  its window, output and prices, offering keys and ChatGPT's sign-in.
  The sign-in is a plan's (`signed_in_plan`, its manage-usage address
  `https://chatgpt.com/settings/usage`), on ChatGPT's public
  registration as jig's live sign-in uses it (jig's
  `domain/providers.md`, 2.2): the issuer `https://auth.openai.com`,
  the entry client `dynamic_agent_client`, the name hint `temper`,
  authorization at `/api/accounts/authorize`, the token at
  `/api/accounts/oauth/token`, revocation at
  `/api/accounts/oauth/revoke` and the published keys at
  `/.well-known/jwks.json`, all on `auth.openai.com` over TLS; the
  scopes `openid profile email offline_access resource.invoke
  chatgpt.tokens.use.direct`, the last of which must be granted; the
  resource `https://api.openai.com/v1`; the redirect
  `127.0.0.1:1455/auth/callback`, its port fixed, as the registration
  requires. A plan's runs use the same endpoint and model as a key's,
  with the grant as a bearer (jig's `protocol/llm.md`, 5.3). The charters
  allow the provider's models in order, the default the first whose LLM
  provider has a usable account (llm.md, section 5; jig's
  `domain/tasks.md`, section 3).
- **Its agent's configuration** (configuration.md, section 5),
  `owned/agent.json`, declares OpenAI's endpoint and model, and
  Claude Code's program, its path and its programs directory, a
  directory the person names; and takes the process tier's profile, the
  sizes its live tier runs a real model with: smith's `standard`, with
  `declared` `memory` 2,147,483,648, `tool_payload` 4,098, `read_window`
  256, `conversations` 1 and `calls_per_response` 1, and `policy`
  `max_turns` 8, `check_timeout` 60,000 ms, `group_stop` 10, `inbox` 8
  and `unacknowledged` 8. Both charters require eight turns; each permits
  modifying work with a 60,000 ms reserve, so the agent's check timeout
  must fit that reserve. The engine requires 4,096-byte messages;
  smith's two-byte rendered separator makes the minimum payload 4,098
  bytes, advertising 4,096 message bytes. Its largest call is then
  4,098 bytes, the larger of that payload and its 4,096-byte outcome
  (configuration.md, section 7, `small` against the agent). smith's
  standard sizes with nothing declared refuse the example's model
  declaration at start.
- **Its `engine.limits`** are jig's small profile with the overrides
  configuration.md, section 7 asks so that the engine's documents, its
  watch room and the forge's paths fit temper's client, and those its
  owner's grants need, and no other:
  - `authority.segments`, `tasks.hold_segments` and
    `tasks.authority_segments`, 8: a name's segments, the forge's
    deepest path;
  - `documents.rows` 2, `documents.lines` 2 and `documents.text_bytes`
    128: temper's client's rows and text;
  - `views.watchers` and `views.watchers_per_party`, 16: the watch room,
    as the root holds it whatever `engine.limits` says
    (configuration.md, section 7);
  - `authority.grants` and `tasks.authority_grants`, 12: its owner
    holds a grant for each of the forge's kinds its work uses, 10
    (above), and small's 4 hold none of that work; with `notes.scopes`
    15, since jig's core admits an authority's grants only with 3 notes'
    scopes more, and refuses every override of the core's otherwise
    (`jig-domain-core`'s `worst_case`);
  - `tasks.authority_bytes` 512 and `tasks.batch` 1, the process
    tier's: the bounds above widen a task's stored row past the small
    profile's store, whose bounds no override changes, so the start
    would be refused (jig's `protocol/store.md`, 5.4); these two bring
    the row back within.
- **Its worker's `limits`** are jig's small profile with the two of the
  hub's that jig's small, sized for any agent, sets past smith's agent
  (configuration.md, section 7, the worker's check against the agent),
  and no other: `hub.accounts` 4, the agent's accounts, where jig's small
  has 8; and `hub.event_bytes` 4,096, the agent's advertised message
  bytes, where jig's small has 65,536. Its `application.limits` is
  `small` with `checkout.message_bytes` 4,098 so the checkout accepts
  the agent's largest call.
- **Its turn's bounds derive from smith's largest turn,** T, the
  agent's `turn_body`, which its configuration derives (configuration.md,
  section 7, the turn's check). The model's reasoning allowance,
  `reasoning_item`, is 65,536 bytes: a plan's model's reasoning items
  stay within it in ordinary use, so a chat on a plan is not held for
  its reasoning, and a person may lower it, with these bounds
  (configuration.md, section 5). Each bound is the least the turn's
  check takes, past jig's small profile's:
  - both hubs, `engine.limits` and the worker's `limits`:
    `hub.turn_bytes` T; `hub.transcript_bytes`
    W + (B − 4) × ⌊W / 4⌋, where W is `core.resume_bytes` and B is
    `size_of::<Box<[u8]>>()`: the window and a box for each turn it can hold;
    `hub.turn_queue_bytes` 8T, the agent's eight unacknowledged turns
    (the worker's check, above);
  - `engine.limits`: `core.resume_bytes` T + 4, one largest turn with
    its length, so that a chat's window resumes whole up to that size;
  - `application.limits`: `root.journal.transcript_bytes` T + 4, the
    resume setting's bound; `root.loads.transcript_bytes` T;
    `root.loads.bytes` and `root.loads.reply_bytes` T and a row's slot;
    `root.loads.read_bytes` 2T, the turn and room as large again for
    the evidence a read carries with it (configuration.md, section 11).

  A test holds the example's and `temper init`'s figures to this
  derivation from their agent, and its writer rewrites them, before the
  memory's writer runs (below):

  ```sh
  SKEIN_CODEGEN_WRITE=1 heavy cargo nextest run --locked -p temper --test startup -E 'test(turn_bounds)'
  ```

  jig's store and its KV bounds and the worker wire's ceilings stay as
  they are: a turn's body is a transcript file's, never a KV value, and
  each figure is within its wire field's ceiling.
- **Its `memory_bytes`** are at least its worst cases, with 10% headroom
  rounded up to two significant digits. The check admits declarations
  from the worst case through 1.25 times it; past either end it says to
  run the writer below. The exact terms live in
  `crates/temper/tests/figures/memory/owned/{engine,worker}/`.
  They hold wherever a person lays it and whatever its forge's records
  hold: each start passes with each string its service's worst case
  counts, and a person's copy changes,
  at its bound (configuration.md, section 7, one worst case per
  service).
  - The engine's counts the forge's API token, its `ApiToken` record,
    and its `git` base, a byte for each of theirs: the token at
    `small`'s `forge_component.token_bytes`, 512, past which the start
    is refused, and the base at an owned forge's longest, `http://`
    with a loopback address and a port, `http://127.255.255.254:65535`,
    28 bytes.
  - The worker's counts its agent's `program` and the configuration's
    path among its `arguments`, two bytes for each of theirs with the
    example's one slot: each at Linux's `PATH_MAX`, 4,096 bytes; and
    Claude Code's path and programs directory, which the host's half
    holds to check a program's login (smith's `protocol/programs.md`,
    section 4), each at the same bound.
  - Nothing else a copy changes moves either: its directory but for that
    path, its other addresses and secrets, its logins, emails, binaries
    and charters, its LLM providers, whose sizes jig's limits bound.

  Channel frames and queues use max(the checked credential value bound,
  the 32-byte source name, the generated Identity bound), independently
  on each side; configured keys include jig's 4,096-byte ceiling, and
  workers reserve it without provider declarations. The program paths
  are retained once, separately from the per-slot spawn command paths.
  `temper-service::startup::memory_requirements` derives both envelopes
  from startup's checked calculations without starting a service. The
  owned startup test checks the two literals against that query's band
  with these strings at their bounds in service settings. Separately,
  it checks that the exact worst case starts and each one-byte deficit
  is refused at startup. Artificial paths are never opened. A
  separate shell test prepares the example with ordinary paths.
  Regenerate the two literals and the exact term figures with:

  ```sh
  SKEIN_CODEGEN_WRITE=1 heavy cargo nextest run --locked -p temper --test startup -E 'test(owned_memory_figures)'
  ```

  Ordinary runs fail when a declaration leaves the band or an exact term
  figure drifts, and never write the example or the figure files.
  Program declarations survive startup; runnable program grants still
  wait for smith's program backend and jig's program sources.
- **Three loopback addresses,** Forgejo's (`listen`, `127.0.0.1:3000`),
  the clients' listener's (`127.0.0.1:7100`) and the webhooks'
  listener's (`127.0.0.1:7101`), are what a host changes to run several
  boxes on one machine, with each box's directories.

### 8.2 `temper init`

`temper init [DIRECTORIES]` writes a deployment that starts, from its
three directories (configuration.md, section 12) and what it finds on
the machine. `temper box` runs it first when the configuration
directory is empty (7.1).

- **Refused,** exit 1, before it writes anything, naming the file: when
  the configuration directory holds `temper.json`, or the data
  directory holds `worker.secret`, `worker.digest`, `demo/llm.key` or
  `demo/bin/git`. It never replaces a file; a person who starts again
  removes them.
- **Its directories:** the data and cache directories made and checked
  as every command makes and checks them (configuration.md, section
  12), and the configuration directory made the same way when absent;
  an existing configuration directory is taken as it is; and the data
  directory's `demo/` and `demo/bin/`, made private (0o700).
- **Its files,** each created new, in this order, the document last, so
  that a configuration directory holding one holds them all:

  | File | Directory | Mode | What it is |
  |---|---|---|---|
  | `worker.secret` | data | 0o600 | 32 bytes from the kernel's random source, as 64 lowercase hexadecimal digits and a newline |
  | `worker.digest` | data | 0o600 | the SHA-256 of the secret's digits without its newline, as 64 lowercase hexadecimal digits and a newline, as `docs/guide/box.md` makes it |
  | `demo/llm.key` | data | 0o600 | the demo model's key (8.4): 32 bytes from the kernel's random source, as 64 lowercase hexadecimal digits and a newline |
  | `demo/bin/git` | data | a link | the demo's git, `temper fake git` (8.4): a symbolic link to the binary |
  | `charters/chat.txt`, `charters/coding.txt` | configuration | 0o644 | the charters' instructions, the owned example's; designed, not built: no longer written, the binary holding them (configuration.md, 13.4) |
  | `agent.json` | configuration | 0o644 | smith's agent configuration (configuration.md, section 5): the owned example's endpoints, models and profile (8.1), and the demo's endpoint and model (8.3); Claude Code's program at the `claude` found on `PATH`, and its programs directory, `programs` in the data directory; with no `claude` on `PATH`, no program, and Anthropic's models by key alone |
  | `temper.json` | configuration | 0o644 | the default document (8.3) |

- **What it finds,** each written absolute:
  - *the binary,* the running `temper`'s own path: the worker's agent's
    `program`, with `arguments` `["agent", <configuration>/agent.json]`
    and `configuration` that same path (configuration.md, section 5),
    and the demo's link's target;
  - *git,* the `git` found on `PATH`: the worker's `git`; with none,
    `null`, written without refusing, and init says so (git.md, section
    3). The demo's link, `<data>/demo/bin/git`, is the fake entry's
    `git` (section 8), so that the demo's git is `temper fake git`
    (8.4) whatever git the machine has, or none;
  - *the person,* forge 3's (8.3): its `login` the name of the effective
    uid's entry in `/etc/passwd` (`geteuid`, then that file), the login
    the person signs in to their Forgejo with; its `email`
    `LOGIN@localhost`; its one org `work`. With no entry, or a name that
    is not a Forgejo login, is one of Forgejo's reserved names (such as
    `admin`), or is the bot's, `temper-admin` or `work`, the login is
    `person`, and init says so: forge 3 waits dormant, so the person's
    login is never a reason the first run does not start;
  - *the bundle:* none to find. `clients.bundle` is always `null`, the
    binary's own (`../client/browser.md`, 2.6): a temper built with its
    browser starts its first run with it. From a binary built without
    one (`TEMPER_BUNDLE=none`), init says the browser is not served;
    the link still signs in a terminal (`temper open`), and a browser
    that opens it is answered the clients' listener's page for no
    bundle (`../client/browser.md`, 2.2), never a bare `404`;
  - *the clients' listener's port:* 8367, when a socket binds
    `127.0.0.1:8367`, else the port the kernel assigns to a bind of port
    0; the port written fixed, as a GitHub forge requires
    (configuration.md, section 3). A later start on a port taken is
    refused, naming it and the document, as any start is;
  - *the forges' ports:* 8368, forge 3's, the owned Forgejo's (8.3);
    8369, the fake forge's, and 8370, the fake LLM's (8.4), each by the
    same rule;
  - *the webhooks' listener's port:* 8371, by the same rule (8.3). The
    five sockets, the clients', these three and the webhooks', are held
    until all five ports are chosen, so that no two are the same; then
    `temper init` closes them, and `temper box`, whose first run ran
    init in its own process, keeps them, and holds each for its step
    from then on (7.1), so that no port it wrote is free between init and
    the step's bind. The port is written fixed, since the setup host gives the
    hook's address from the document at its start (5.3); a listener
    bound to port 0, its hooks given the address it reports, is later.
- **What it says,** on stderr: `init: wrote <path>` for each file;
  `init: this temper has no browser (built with TEMPER_BUNDLE=none):
  the browser is not served` from a binary built without its bundle;
  `init: no git on PATH: a real forge's repositories need git
  2.41 or later, set as workers[].git` when there is none; `init:
  LOGIN cannot be a Forgejo login: forge 3's person is person` when
  the login falls back (above); `init: uid UID has no login in
  /etc/passwd: forge 3's person is person` with no login; and last,
  `init: start it with temper box`, with the flags init was given,
  then `init: after starting, continue with ChatGPT or add an OpenAI API
  key from the accounts page`. Its
  stdout is empty; exit 0.
- **Nothing else:** no account, no forge credential, no repository and
  no store. The engine's first start makes its store; accounts, forges
  and repositories are the client's (credentials.md, section 3;
  `../client/forge.md`), and the demo's account and repository the
  demo's start's (8.4).
- **Its code:** `temper-init`, step code (section 11): given the facts
  above, git's path or none and the person's login among them, the three
  directories, the secret's 32 bytes and the demo key's 32 bytes, it
  answers the files, each with its directory, its name, its bytes and
  whether it is private, and the person's login, fallen back to `person`
  or not; it reads, writes and binds nothing. The binary's `init` finds
  the facts, checks the refusals and writes the files, on skein-io.
- **Checked:** temper-init's step tests check that its `engine.llm` is the
  owned example's with the demo's provider (8.3), that the digest is the
  secret's, the files' names, directories and order, and forge 3's
  entries with the person's login found and fallen back. The binary's
  crate tests, on scratch directories, check that the document init writes
  passes configuration.md, section 8, as `temper serve` checks it, with
  the agent's configuration it writes, at the fixed ports and at kernel's
  ports, the five always distinct, with and without `claude` and git,
  `clients.bundle` always `null`; each refusal writing nothing, the person's fallback login among
  them; the files' modes; and the link's target. A box on a fresh init,
  among those tests, reaches `temper serve`'s sign-in link with forge 3
  dormant: no `forge-3` directory and nothing in the cache. The process
  tier starts a box on empty directories (13.4).

### 8.3 The default document

A working minimum, for the mechanics and a person's first run; good
defaults for a person's real work are open (section 16).

- **Its forges,** each by its stable number (configuration.md, section
  3):
  - 1, the demo forge, Forgejo, its members its setup's (section 8):
    `api` `127.0.0.1:<the fake forge's port>`, plaintext, and one git
    identity whose `login` is the bot's, `temper`;
  - 2, GitHub, at GitHub's addresses, with one git identity whose
    members are GitHub's defaults, not connected until the person
    connects it from the client (section 17);
  - 3, the person's own Forgejo, its members its setup's (section 8):
    `api` `127.0.0.1:<its port>`, 8368 (8.2), plaintext, `git`
    `http://127.0.0.1:<its port>`, and one git identity whose `login` is
    the bot's, `temper`; dormant, not connected until the person sets it
    up from the client (6.5). Its handover is `forge-3/admin_token`; its
    access and git value are records, with no other credential file
    written or read.

  GitHub and the owned Forgejo are in the document from the first run,
  so that connecting either from the client needs no restart. A start
  resolves each forge's `api` name once (README.md, section 14); a name
  that does not resolve, GitHub's on a machine without DNS, refuses no
  start: that forge starts not connected, and each connect of it,
  restored or asked, is answered unreachable, naming the host, until a
  start that resolves it. The other forges start as they would. A
  forge so held keeps its connection and its setup entries, built with
  the address `127.0.0.1:0`, which no listener can hold, and temper sends
  neither a connect nor a setup request for it: the placeholder never
  reaches a socket.
- **Two `setup` entries:**
  - forge 1's, fake, in `demo/forge`, its `git` the demo's link (8.2),
    its bot `temper`, its `owner` `playground`, its one repository `widget`,
    landing on `main` (8.4);
  - forge 3's, owned, in `forge-3`, `"start": "asked"` (7.1): its
    `forgejo` and `runner.binary` `"fetched"` (6.6); its runner the
    owned example's, `temper-host:host`, capacity 1, and its `path`;
    its `listen` `127.0.0.1:<its port>`; its bot `temper`; and its
    person the one init finds, `LOGIN@localhost`, with the org `work`
    (8.2).

- **The webhooks' listener,** `application.webhooks.listener`,
  `127.0.0.1:<its port>` (8.2). Each Forgejo's `WebhookSecret` is a
  record, drawn by the engine's reconcile. Adoption makes the
  repository's hook; the fake's start-time adoption hooks its seeded
  repositories (8.4). GitHub's hints come from its App's delivery log.
- **Its LLM providers:** the owned example's `engine.llm`, whole (8.1),
  with no account; and 3, Demo, the demo model's (8.4): one endpoint,
  `demo`, at `127.0.0.1:<the fake LLM's port>`, plaintext, OpenAI's
  dialect on its public route; one model, `demo`, priced as the owned
  example's model is, 1 per 1,000,000 for input, cached and output (jig
  and smith refuse a price of 0); its window 131,072 and its output
  8,192, so that smith's room at the owned threshold, 4/5, still offers
  the owned message bound, 4,098 bytes, and a request at the threshold
  stays within the fake's request bound; its source a key. The agent's
  configuration declares it too (8.2).
- **No project** (`../draft/first-run.md`, 2.4): the playground's
  start makes the playground's (8.4), and the person's first repository
  theirs (6.7). One policy, `engine.policies` 1 (jig's
  `protocol/configuration.md`, section 3), which each project made
  starts from: its owner's grants, the rules' ceiling and its
  projections the owned example's, on the pattern below in place of
  the org's. The deployment's landing rule, `landing.deployment`,
  covers every repository of every forge, the pattern `{"segments":
  ["forge"], "last": "open", "value": ""}`, `enforce`d, with `ci` and
  `up_to_date`, and no gate or approval: a change lands on green CI
  with no person in between, in every project, seeded or made
  (configuration.md, section 8). The document seeds project 1 with that
  policy, and the playground's start adopts into it (section 15); the
  first repository goes into a project made at runtime by jig's
  `MakeProject` (6.7). Designed, not built: the policy and the landing rule are
  the binary's stock settings, policy 1 and the deployment's rule 1, and
  the document names neither (configuration.md, 13.4); `temper init`
  writes no `engine.policies`, `engine.rules` or `application.landing`.
- **Its acts:** `engine.acts` (jig's `protocol/configuration.md`,
  section 3) grants the forge connector, 1, the demo end's acts
  (8.4), `{"connector": 1, "may": ["close", "remove_account"]}`,
  and no other connector any. The
  document's first owner, whom the grant needs, is its `owners`' first.
  A document written before the grant still serves: `temper serve`
  starts on it, and only the demo's acts are refused, *not offered*.
  A start then fails *not granted*, its message naming the missing
  grant and the line to add to `engine.acts`, the one `temper init`
  writes; the end's acts are kept and named in serve's log until a
  start's document grants them.
- **Its charters and templates,** the owned example's, but both
  charters' models end with `demo`: chat, 1, and coding, 2, the one the
  forge's procedures start agents' work under (`application.charter`);
  the templates `chat` and `goal`. A task with no model chosen takes the
  first of its charter's models whose provider has an account serving
  its payer (jig's `jig-domain-core` `llm.rs`), so on a first run, whose
  one account is the demo's, a chat and the coding agent of the change
  it delegates both run on the demo model, with no pick, until the
  person adds an account. jig resolves that default once, at the task's
  Create, and records it: a chat started before the account stays on
  the demo model, and a chat started after takes the charter's first,
  `gpt-6-luna` once an OpenAI key serves it. The composer's line says
  which, before the start, from jig's `resolved_default` preview in
  `me`, never by guessing from catalogue order (`../client/README.md`,
  5.5). The request keeps its omitted choice; the preview does not turn
  it into an explicit one. Designed, not built: the charters, the
  templates and the forge's charter are the binary's stock settings,
  their models every model `engine.llm` configures in the document's
  order, the demo's last as `temper init` writes it; the document names
  none of them (configuration.md, 13.4), and a person changes them from
  "How work runs" (`../client/README.md`, 5.13).
- **One worker,** in the process, its `root` `workspaces` in the data
  directory, its `git` the machine's, or null (8.2); the deployment's directory
  `deployment` in the data directory; the clients' listener
  `127.0.0.1:<port>` (8.2).
- **Its limits:** the owned example's (8.1), but room for its three
  forges: `forge.repositories` and `forge.client.repositories` 3, and
  `forge.output` 82. Its `memory_bytes` are at least its own worst cases,
  with 10% headroom rounded up to two significant digits, checked in the
  band from the worst case through 1.25 times it, as the owned example's
  are (8.1). Its own strings are at their bounds, the fake entry's `git`,
  which the engine names to the worker, among them; the two literals of
  `temper-init`'s forgejo-elsewhere/demo document are written and checked
  the same way. The exact terms live in
  `crates/temper/tests/figures/memory/init/{engine,worker}/`.
- **`application.demo`,** the demo's (8.4).
- **Its box's steps:** `temper serve`; the demo's group, the fake LLM,
  forge 1's preparation and its fake, dormant until the person chooses
  the playground (7.1, 8.4); and forge 3's group, dormant until the
  client asks for it: its fetch, its preparation, Forgejo's web and
  the runner's daemon; the engine takes the handover and reconciles
  (6.5).

### 8.4 The demo

A person's first run is a demo (`../draft/first-run.md`, 3.5, 3.6 and
5, step 3): once the person chooses the playground, the box runs a
fake LLM and a fake Forgejo from temper's own binary, the engine sets
itself up on the fake as on an owned one, and makes the playground's
project, adds the demo model's key and adopts the demo's repository
itself (the demo's start, below).
Its bar: one suggested request landed
end to end on the demo model, by the engine's own loop with nothing
staged. The chat delegates a change on `playground/widget`; its coding agent
edits a file; the worker commits and pushes the branch with `temper
fake git`, on the fake forge's git route; the change opens its pull
request, passes the fake's CI and lands into `main`; and the chat says
so. The other suggestions answer in text. The engine hears the fake by
its deliveries to the box's webhooks' listener (8.3), and polls beside
them as it does any Forgejo.

- **`application.demo`,** or null for none:

  ```
  "demo": {
    "llm": {"listen": {"socket": "127.0.0.1:8370"}, "provider": 3,
            "key": "demo/llm.key",
            "asked": [{"cue": "Add CSV export to widget",
                       "turns": ["..."]}],
            "cued": [{"cue": "Export widget's table as CSV",
                      "turns": ["..."]}],
            "otherwise": ["..."]},
    "forge": 1, "project": {"name": "Playground", "policy": 1}}
  ```

  | Member | Type | Bounds |
  |---|---|---|
  | `llm.listen` | an address, jig's format | a socket address on loopback; unique among the deployment's listeners and owned and fake forges |
  | `llm.provider` | u32 | the number of one of `engine.llm`'s providers (configuration.md, section 2), whose endpoints are each at `llm.listen`, plaintext, OpenAI's dialect on its public route, and whose source is a key alone |
  | `llm.key` | a data path (configuration.md, section 12) | a secret's file, 0o600 (section 9): the key is its bytes less a final newline |
  | `llm.asked` | a list of `{"cue", "turns"}` | at most 8 with `cued`'s; each `cue` non-empty text of at most 256 bytes, unique across `asked` and `cued`, the words of a request the client suggests (`../client/README.md`, 4.8 and 5.6); `turns` 1 to 8 turns (below) |
  | `llm.cued` | a list of `{"cue", "turns"}`, optional, none when absent | as `asked`'s; the scripts the fake plays and the client never suggests, an agent's, cued by its brief |
  | `llm.otherwise` | a list of turns | 1 to 8 |
  | `forge` | u16 | the `forge` of one fake `setup` entry (section 8) |
  | `project` | `{"name", "policy"}` | the project the demo's start makes (below): `name` a project's name in jig's shape (jig's `domain/core.md`, 6.1), `Playground` as `temper init` writes it; `policy` the number of one of `engine.policies` (jig's `protocol/configuration.md`, section 3); designed, not built: of the policies offered at its start, the stock policy 1 unless changed (configuration.md, 13.4) |

  The scripts, `asked`, `cued` and `otherwise` together, fit skein's fake LLM's
  `script_bytes` (skein's `fake-llm.md`), which `temper-fake` sets.
  Any other member is refused, naming it (configuration.md, section 1),
  `client` among them: a document `temper init` wrote while the demo
  had its seed is refused at its next start, naming
  `application.demo.client`, until the member is removed.
- **A turn** is skein's `Turn` (skein's `fake-llm.md`, section 2), in
  either of two spellings:
  - *a text:* one text line, finishing `stop`, its tokens its bytes
    divided by 4, rounded up;
  - *an object,* `{"lines", "finish", "tokens"}`, the QA loop's and the
    demo's: `lines`
    1 to 8, each `{"text"}`, non-empty text, or `{"call", "arguments"}`,
    a tool's name of 1 to 64 ASCII letters, digits, `_` and `-` and its
    arguments a JSON object, which `temper-fake` writes compact as the
    call's bytes; `finish` one of `stop`, `tool_calls`, `length` and
    `content_filter`; `tokens` a u64, at least 1.

  Each is checked as skein checks a script's turns (its section 2.3).
  No turn holds a hold, a failing line or opaque bytes.
- **Its words,** as `temper init` writes them (8.2), the playground's,
  as the person meets the demo (`../draft/first-run.md`, 2.3; the
  client's phrasebook, `../client/README.md`, 5.6): one `asked` script
  for each request the client suggests in the demo project
  (`../client/mockups/first-run.html`), "Add CSV export to widget", "Add
  a dark theme to widget" and "Fix widget's empty search state"; one
  `cued` script, for the coding agent of the change the first
  delegates; and an
  `otherwise` that says it is the scripted model, the playground's,
  which answers only the
  suggested requests, and how to connect a real one from the client's
  accounts page. Each turn is an object, its tokens its lines' bytes
  divided by 4, rounded up. Their content is the demo's follow-up
  (`../draft/first-run.md`, section 8).
  - *"Add CSV export to widget",* the chat's, three turns, each
    finishing `tool_calls`:
    1. the text "I’ll delegate this change in playground/widget. It
       will push a branch, open a pull request, check CI and land into
       main on Playground forge.", then `delegate`, one item: the change procedure,
       `{"kind": "procedure", "connector": 1, "code": 2}`; its spec's words
       `Export widget's table as CSV`, and its parameters the repository,
       `{"kind": "resource", "connector": 1, "resource": 1}`, the fake's
       `playground/widget` (below), and the bytes `main`; its contract a
       change on connector 1, kind 1, in at most 128 words; its
       authority the one the real loop's landing story gives its change
       (`tests/real-loop`'s `landing.rs`), its grants on the project's
       pattern (8.3) and its delegation to the coding charter's agents,
       `{"kind": "agent", "number": 2}`;
    2. `{"call": "wait", "arguments": {}}`;
    3. once the change's result wakes the chat, the text "CSV export is
       available from widget’s table. The change landed on Playground
       forge. This was a scripted change.", then the same `wait`.

    The two texts are the mockups' first run's
    (`../client/mockups/world-first-run.js`), with their typographic
    apostrophes. The first
    goes with the `delegate` it announces, since the chat's page shows a
    message's text beside its calls (`temper-client-view`'s chat page).

    It never calls `finish`: the chat stays open, its composer with it,
    and the chat's 32-byte report bound (`engine.templates`' `chat`)
    never applies.
  - *`Export widget's table as CSV`,* the coding agent's, cued by the
    delegate's words, which are the change's title and so its agent's
    brief; the chat's request never reaches that agent's system text,
    nor do these words reach the chat's, so the two cues match apart.
    Three turns, each finishing `tool_calls`: `read` of `src/main.rs`;
    `edit` of it, `{"path", "edits": [{"old", "new"}]}`, its `main`
    printing the table as CSV when run with `--csv`, a header line
    `name,count`, then a line for each row; and `{"call": "finish",
    "arguments": {"form": "change", "fields": {"title": "Add CSV export
    to widget", "body": …}}}`. The worker commits and pushes what it
    left, and the procedure opens, checks and lands the change
    (`../domain/forge.md`); the coding charter's agent writes no git.
  - *The dark theme, the empty search, and `otherwise`,* one turn each:
    its text line, answering what it would change and why, finishing
    `stop`. The chat is ended by its requester (`../domain/agent.md`,
    4.6), so its run is smith's Open mode, which offers no `finish`: a
    turn that ends with text is its reply (smith's `domain/run.md`,
    section 6), committed with the turn, and the chat waits for the
    person's next words, its composer with it, as the CSV's does with
    `wait`.

  What the scripts cannot know, they do not say: the task's number,
  which the client's card for the change shows; and how the change
  ended, so the CSV's third turn says it landed even when the task's own
  row says otherwise. A message the person sends while the CSV's chat
  waits answers its wait and plays its third turn early, and one more
  is past its turns, answered "done", nudged once, and fails the run. A
  second CSV request, once the first landed, finds `main` past the
  edit's `old` and its change fails, until a restart seeds the fake
  again. `temper-init`'s tests check the scripts as data: each cue
  matched apart in its own system text and absent from the others',
  within `temper-fake`'s bounds; the process tier's first run plays
  them (13.4). The real loop's landing story already plays a cued
  chat's `delegate` to its landing on skein's peers, so the demo adds
  no story there.
- **`temper fake llm DIRS`,** a long step: skein's fake LLM peer, run
  `without_records` as a peer serving a person is (skein's
  `fake-llm.md`), its domain `Domain::played` with `asked` then `cued`
  as its cued scripts and `otherwise` as its fallback, no asked script and no
  menu. A chat's opening words are its run's brief, which smith puts in
  the system text, and the one user message is smith's own opening, so
  a request's cue is in the system, where skein's cued scripts look and
  its asked ones do not (skein's `fake-llm.md`, 2.4). A cued script's
  turns count every assistant message of the conversation, a tool's
  call among them, so a chat's first answer is its script's first turn
  and each result or message that wakes it plays the next. A one-turn
  script replies and waits, as the CSV's does: the chat stays open
  (above). A message past a script's turns is answered "done". It reads
  the key file before it binds, and refuses to start without it, naming
  the file; it accepts that one credential, and refuses every other
  request as the provider would. It serves `llm.provider`'s models, at
  their windows and outputs; no failure and no latency is configured,
  and its limits are `temper-fake`'s constants.
- **`temper forge prepare DIRS N` on a fake entry,** a once step: its
  directory made private; `admin_token` written when absent, 20 bytes
  from the kernel's random source as 40 lowercase hexadecimal digits,
  as Forgejo's tokens are. The fake reads it before it binds, so the
  handover is the preparation's, not the fake's own at its start, as
  the draft had it: no step races `temper serve` for the file.
- **`temper fake forge DIRS N`,** a long step: skein's fake forge peer,
  `without_records`, at `fake.listen`. It reads `admin_token` before it
  binds, refusing to start without it, naming the file, and leaves the
  file for `temper serve`. Its seed:
  - *temper's admin,* `temper-admin`, a site admin of the setup's
    routes, its one token the handover's, with every scope (skein's
    `fake-forge.md`, section 4);
  - *the owner,* `fake.owner`, a user of the domain, numbered apart from
    the setup's accounts (skein's `fake-forge.md`, section 5);
  - *its repositories,* `fake.repositories`, each the owner's, in the
    protocol's `config.repositories` and the domain alike, numbered
    from 1 in the list's order at every start, so the box's
    `playground/widget` and the QA loop's `fake/widget` are each 1; on its
    `landing` branch, one commit of `temper-fake`'s constants, its
    files, author and time alike, so its head is the same commit at
    every start: `README.md`; `src/main.rs`, whose `main` prints
    widget's table, three rows of a name and a count, the file the
    demo's change edits; and `.forgejo/workflows/check.yml`, which
    describes the check the fake's CI stands for and is not run;
  - *its CI,* `Checks` with one context, `check`, with no latency,
    silence or rerun: on a repository with a cue, the domain's `Cue { path, green
    }` (skein's `fake-forge.md`, 2.3), a head whose file at `path` holds
    `green` passing and any other failing; on one without, every head
    passing. The box gives none; the lab's form gives each repository's
    (below).

  The bot is reconcile's, and its access to a repository the adoption's
  collaborator grant, which reaches the fake's domain (skein's
  `fake-forge.md`, section 4): the seed grants nothing, and the
  engine's setup host grants it, by `Adopt` (below). It serves its
  git route among its routes (skein's `fake-forge.md`, section 4), and
  no git smart HTTP; it keeps no state across its starts.
- **The fake's start-time adoption,** in `temper serve`'s setup host:
  after the records' load, the root's restore, the handover and
  reconcile (6.5), the demo's forge's at its group's `started` (7.1),
  one `Adopt { forge, repository }` for each of
  `fake.repositories` not already adopted, numbered from 1 in the list's
  order as the fake numbers them. A repository restored by the root is
  already adopted: reconcile checks its access, and start-time adoption
  skips it. The plans run one after another (5.1), with the same given
  values as owned (5.3), the admin's token and webhooks' secret
  from records, and the deployment's listener when configured. Each
  makes the bot a collaborator with write and the repository's hook; the
  forge connects from records once they all end done. A stop leaves the
  forge not connected, named by its step; the engine and every other
  forge run on. The demo's start adopts the repository into the
  project once the forge is connected (below). `temper forge fetch` and `run` refuse a fake, "it is fake,
  not owned".
- **Its deliveries follow the setup's hooks** (skein's `fake-forge.md`,
  section 4): each event goes to each hook a repository's setup holds
  that takes it, signed by that hook's secret. A repository's hook is
  made at adoption when the deployment has a webhooks' listener
  (section 10), at `http://<listener>/hooks/<number>`, so the fake
  reaches serve's listener as a Forgejo does: a push, a pull request's
  change, CI's end and the merge each send one. The box's listener is
  on loopback (8.3), and its fake draws no loss and no latency; it
  records no delivery, being `without_records`. The QA loop's serve
  has one too, on loopback.
- **`temper fake git`,** no step: the demo forge's git, which the
  worker spawns for the fake's repositories by the program their
  records name, the fake entry's `git`, the demo's link (8.2; channel.md,
  section 4), in place of `workers[].git`, as it spawns real git
  (git.md, section 5), and which it checks at its first use as it checks
  real git, by `git version` (git.md, section 3). A real forge's
  repositories, on the same worker, take the machine's git. It is the
  fake git program (`../domain/testing.md`,
  4.3): the fake checkout's `.git` in the worker's working trees; its
  remote the fake forge's git route, at the host of the clone URL the
  engine gives; its credential the bot's, from git's
  `http.extraHeader`; its store the fake's domain, so a push is the
  head the engine reads.
- **Its programs' flags,** for the box and the QA loop's runner alike
  (`../draft/qa-loop.md`, sections 4 and 11), after the operands:
  - `--ready PREFIX`: once its listener is bound, the program writes
    PREFIX, then the bound address as `127.0.0.1:<port>`, then a
    newline on stdout, flushed; nothing else is on its stdout;
  - `--seed N`: its domain's seed, a u64 in decimal (skein's
    `fake-llm.md`'s `Domain::played`; skein's `fake-forge.md`'s
    `Service::new`), 0 when absent, as the box runs it; in a document's
    form, its credentials' too (below);
  - `--observations FILE`, the fake forge's, and in a document's form
    the fake LLM's, whose lines are its queries
    (`../domain/testing.md`, 4.1): FILE created new at
    0o600, its first line a checkpoint, `{"observations_lost":0}`,
    written before the ready line, and each observation its domain
    makes (skein's `fake-forge.md`, 2.4) drained after each pass, as a
    referee drains it, and written to FILE in skein's encoding, one
    JSON line each in drain order. A pass that observed or lost any
    ends its lines with a checkpoint, `{"observations_lost":N}`, the
    count lost so far, and the program writes one more at its stop, so
    FILE is whole up to its last checkpoint from its ready line on, a
    copy taken while it runs as much as the file, and a stopped
    program's last line is one. Nothing
    of a pass is sent, no reply and no delivery, before its lines'
    write completes, so a reader that has seen a reply, or what a reply
    caused, finds in FILE what the pass observed. Without the flag, the
    program drains and drops them, and sends as it serves.

  A write failure stops the program with exit 1. After the first failed
  write it writes nothing more to FILE, including at stop: it drains and
  drops observations and refuses every later pass's sends. A drain past
  its byte bound has the same rule. FILE retains its earlier writes and
  whatever bytes the failed write left, with no later checkpoint to
  certify lines it lost.

  Both peers run `without_records`, with the flags or without them,
  but the fake LLM given `--observations`, which records its queries.
- **From a document of its own,** `temper fake llm --document FILE` and
  `temper fake forge --document FILE`, the QA loop's form, which reads
  no deployment: FILE is a fake's configuration, a JSON object, as
  `temper-fake` derives it from the deployment's document in the box's
  form, with what a run needs beside it. The LLM's:

  | Member | Type | Bounds |
  |---|---|---|
  | `listen` | an address, jig's format | a socket address on loopback, port 0 for the kernel's |
  | `key` | an absolute path | the key's file, which the fake writes (below) |
| `added_keys` | a list of absolute paths, optional, none when absent | at most 15, each distinct and other than `key`'s: further keys' files, which the fake writes (below), the provider's accepted credentials 1 onward in the list's order, `key`'s being 0 |
  | `tls` | `{"root"}`, or null for plaintext | `root` an absolute path: the test root's file, which the fake writes (below) |
  | `models` | a list of `{"name", "window", "output"}` | 1 to 8, each as a provider's model's members are (configuration.md, section 2) |
  | `cued` | a list of `{"cue", "turns"}`, each with `inputs` optional | at most 8, as `llm.asked` is; each cue looked for in the system text (skein's `fake-llm.md`, 2.4) |
  | `asked`, `otherwise` | as `llm.asked` and `llm.otherwise`, each of `asked` with `inputs` optional | the same |
  | `control` | an address, jig's format, optional | a socket address on loopback, port 0 for the kernel's; required when a turn holds a piece |

  A script's `inputs` are its turn rules (skein's `fake-llm.md`, 2.4):
  a list of `{"cue", "turn"}`, 1 to 16, in the order they are tried,
  `cue` text of at most 256 bytes, empty in the last rule and only
  there, and `turn` the index of one of the script's turns; without
  them, the script counts its replies as before. A turn's lines may
  also be `{"pieces"}`, here alone and never in a deployment's
  document: 1 to 8 pieces, each `{"text", "hold"}`, `text` non-empty
  and `hold` a name of 1 to 64 bytes or null (skein's `fake-llm.md`,
  2.2). The fake releases a hold by its `control`, a listener of its
  own beside the provider's: `POST /release/NAME` answers 204 once the
  release is the domain's event, whether or not a reply is held at
  that name, and any other request 404. A document whose turns hold a
  piece and that names no `control` is refused at the start, naming
  it, since nothing could release the hold (skein's `fake-llm.md`,
  2.2).

  A turn's lines may also hold a failing line, `{"fail"}`, here alone
  and never in a deployment's document: `{"error", "credentials"}`,
  skein's `Line::Fail` (skein's `fake-llm.md`, 2.3), with its report
  the domain's default. `error` is one of `"overloaded"`,
  `"unavailable"`, `"context_too_long"`, `"unauthorized"`,
  `"refused"`, `"unsupported"` and `"invalid_request"`, or
  `{"rate_limited": {"retry_after_ms"}}` or `{"exhausted":
  {"retry_after_ms"}}`, `retry_after_ms` a u64 of milliseconds, at
  least 1, null in an `exhausted` for a limit without a reset; skein's
  error classes are translated one for one, so that a class skein adds
  is a build error in `temper-fake` the day it is added.
  `credentials` is a list of 0 to 16 distinct indices of the
  document's accepted credentials (`key`'s 0, then `added_keys`'),
  each below their count, an empty list failing every call. Where a failing
  line falls in its turn and whose calls it fails are skein's (its
  2.3): a refused document is refused at the start, naming the turn.

  The forge's:

  | Member | Type | Bounds |
  |---|---|---|
  | `listen` | an address, jig's format | a socket address on loopback, port 0 for the kernel's |
  | `admin_token` | an absolute path | the handover's file, which the fake writes (below) |
  | `owner` | a Forgejo login | other than the bot's and `temper-admin` |
  | `repositories` | a list of `{"name", "landing", "ci"}`, each with `id` and `tree` optional | as `fake.repositories` are; `ci` `{"path", "green"}`, or null for every head passing: `path` a file's path in the repository, relative, none of the seed's, and `green` non-empty text, both within the domain's content limits; `id` and `tree` as `../domain/testing.md`, 4.2, gives them |

  The box's derivation gives `cued` none, `tls` and each `ci` null.
- **Its outputs,** in a document's form, where the box's fakes read the
  files `temper init` and the preparation drew: the fake writes each
  file the document names before it binds, so that each exists before
  its ready line, created new at 0o600; a file already there refuses the
  start, naming it. Each file's directory must exist, but the admin
  token's, the forge's directory (section 3), which the fake makes
  private when absent and refuses, naming it, when it is not private, as
  `temper forge prepare` does. Each key, `key`'s and `added_keys`', and the admin token are each 20
  bytes as 40 lowercase hexadecimal digits, from skein-lib's `Rng`
  seeded by `--seed` and a constant of each credential's (an added key's its index's), a stream apart
  from the domain's draws, so the runner draws none
  (`../draft/qa-loop.md`, section 4). A seeded credential is no secret
  from whoever holds the run's record: it opens only that run's loopback
  processes, which end with it, which is why the box draws its own from
  the kernel instead. `tls.root` gets
  `skein_fake_peers::transport::ROOT`'s bytes, and the fake serves the
  fixed chain that root signs, whose leaf names `skein.test` and
  `127.0.0.1` (skein's `fake-llm.md`, section 3); an endpoint reaching
  it names `skein.test` as its server and trusts that file alone
  (configuration.md, section 12), as the QA loop's serve and agent do.
  The fake removes none of them at its stop; the engine takes and
  removes the handover (6.5), and the run's directories go with the run.
- **A runner** starts the fakes first, learns their addresses from their
  ready lines, then writes temper's document naming them and the
  handover the fake wrote, and starts `temper serve`.
- **`temper serve`** takes the fake's handover and reconciles from
  records (6.5), runs its start-time adoption from those records, then
  connects (8.4): under the box, the demo's forge once its group has
  started (7.1).
- **The demo's state** is the engine's, one record of temper's store,
  `Demo`, keyed by the demo's forge (`store.md`, section 2): none, *not
  started*; *running*, from the person's choice of the playground; and
  *ended*, for good, once real backends replace the demo
  (`../draft/first-run.md`, 2.3), written by the demo's end (below).
  Nothing else holds it: the box keeps no record of the
  demo's group (7.1), and the client reads it with the demo
  (`../client/README.md`, 4.8). The client's `DemoAnswer` carries
  `DemoState::Ended` with the configured provider, forge and project when
  retired; an empty answer means the deployment has no demo. A reload
  retains that identity without offering or starting the playground.
  Each start of the engine reads it, so a
  restart and the moment of the choice look alike: running, the box
  link asks for the demo's group at serve's first link, as the start
  below does, and the group's `started` takes the handover and
  reconciles (6.5); not started or ended, the demo's fakes stay
  dormant. `temper fake seed`, a link step that made the start's acts
  through `temper ctl` calls, goes with it.
- **The demo's start,** the client's `StartDemo`
  (`../client/README.md`, 4.6 and section 8), of the deployment's
  scope, admitted as `SetUp` is. The forge connector reads the record:
  with no demo, or one ended, it refuses, *not offered*. Otherwise, in
  one decision, it writes *running* when the record is none and, unless
  the demo's forge is connected or connecting, asks for the demo's
  group (`start FORGE`, 7.1), which the box link holds while another
  group starts and sends once that one answers; without a pipe it asks
  nothing, the fakes being the runner's (7.4). Then it
  makes the seed's acts itself, each reading first and done when it
  finds itself done:
  1. the demo's project: the `Demo` row's, once a start has made it
     (`store.md`, section 2); else a project named `project.name`,
     which a start cut short after making it leaves; else made, by
     jig's `MakeProject` of that name and `project.policy`, the person
     its owner and only party (`../draft/first-run.md`, 2.4). Its
     number is then written to the row;
  2. the demo model's key, `llm.key`'s bytes less a final newline, as
     the deployment's account of `llm.provider`, unless the deployment
     holds an account of that provider. The service reads the file at
     each start and carries its bytes as a pasted value's
     (`../client/README.md`, section 8); a start that adds no account
     leaves the value unused, erased before the answer;
  3. once the demo's forge is connected, `fake.repositories`' first
     adopted into the demo's project, from `forge`, the fake's `owner`, on its
     `landing`, with the adoption form's defaults
     (`../client/README.md`, 4.4), unless the project has it.

  Each act is the deployment's, the forge connector's own (jig's
  `domain/people.md`, 5.3), granted by the document's `acts` (8.3),
  never the person's: no sign-in admits it and no role of the person's
  bounds it. The account is `AddKey { provider, join: None, digest }`,
  carrying the start's value, under a key derived from the start's key
  and `account`; the adoption is `Adopt { project, adoption }`, the
  forge connector locating the repository and answering the core's ask
  as it does a party's adoption, under a key derived from the start's
  key and `adoption`. The account is asked in the start's own
  decision, so no restart comes between the two. The start
  is answered once all three are done, *running*, or at the first that
  fails, *start failed*, naming why: the group's `failed` with its
  `WHY`, the project's refusal, the account's refusal, the adoption's
  refusal, the acts not granted (*not granted*, naming the `acts` line
  to add, above), or the forge
  not connected within `setup.deadline_ms` of the start. A start while
  the demo runs makes what is not done, and is answered the same: a
  second start, or a start that failed, sent again, adds and adopts
  nothing twice. A start while another is under way waits for it and
  takes its answer. The answer is kept in memory alone, as `SetUp`'s
  is (`../client/README.md`, section 8): the same key while it is held
  answers the same outcome, and after a restart, unknown.
- **A start a restart cut short** leaves the record running and the
  adoption undone, since it waits for the forge to connect. The engine
  mends it with no client: each time the demo's forge connects while
  the record reads running, no start is under way and the project does
  not have the repository, the forge connector makes the adoption
  itself, `Adopt` under a key derived from the demo's forge and the
  repository's number alone, so that every start of the engine sends
  the same act and jig makes it once. Its refusal is named in serve's
  log, and the next connection tries again. The account is never
  re-made: the start made it in its own decision, and a person who
  removed it since removed it for good.
- **A restart** of the box starts no fake until serve asks: serve's
  start reads the demo running and asks for its group (above), its
  preparation draws a new handover, and the engine takes it at
  `started` and reconciles, which makes the bot, its tokens and each
  adopted repository's access again (5.3, step 6). The account and the
  adoption are jig's records and stay; no act is made again but the
  adoption a start left undone (above), and the end's acts still kept
  (below). The fake
  seeds its repositories the same at every start, so a worker's clone
  of one still matches it; a branch an agent pushed is lost with the
  fake's state, and the engine's records of it outlive it, until the
  fake keeps its state across starts (below).
- **The demo's end,** for good (`../draft/first-run.md`, 2.3), the
  forge connector's, in one decision: the first time it is told a
  configured forge other than the demo's connected (`forge`; never
  every fake), while the record is none or running, it
  1. writes *ended*: a demo never started is ended the same way, and
     never starts;
  2. ends the demo's forge in its domain (`../domain/forge.md`, 4.1):
     no list shows it, the routing serves it no call and drops its
     hints (`forge.md`, 8.1), and the setup host sets it up no more;
     and with it `fake.repositories`' repository, which leaves the
     project's repositories while its tasks, chats and landings stay
     readable;
  3. under the box, when the demo ran, asks the box link for `stop
     FORGE` (7.1), which the link holds while another group starts and
     sends once that one answers, as it does a start; the box closes
     the group's steps, the fake LLM's with them, and answers `stopped
     FORGE`, and a `failed` is named in the log alone. Without a pipe
     it asks nothing: the fakes are the runner's (7.4), and stopping
     them is the service manager's;
  4. answers a start under way *not offered*, as it answers every
     start after;
  5. ends what the demo left, as the deployment's acts (jig's
     `domain/people.md`, 5.3), each kept as a `DemoAct` row of temper's
     store (`store.md`, section 2) under its key, written in this
     decision, and asked in it:
     - *the demo's chats,* each closed by `Close { project, task, grace,
       reason }`, its reason `Ended with the playground`, its grace the
       chat template's (the template whose flags mark a conversation,
       jig's `engine.templates`), its key derived from the chat's
       number and `close`. The demo's chats are the live conversations
       of the demo's project whose requester is a person, as they stand
       at the end: until the end no forge but the demo's is connected,
       so no chat of the project works on another forge's repository,
       while a chat opened after it, on the person's first repository
       in the same project, is not the demo's and stays open. The
       selection is the end's, kept in its rows, never read again later;
     - *the demo model's account,* the deployment's account of
       `llm.provider`, when it holds one, removed by `RemoveAccount {
       account }`, its key derived from the account's number and
       `remove`.

     The client's page of a chat so closed reads it cancelled by the
     deployment, "Cancelled HH:MM · Ended with the playground"
     (`../client/README.md`, 3.5).

  Each start of the engine reads the record: ended, the demo's forge
  is ended from the start and its group never asked, so a restart and
  the moment of the end look alike. The client's read answers none
  (`../client/README.md`, 4.8), so no page shows the playground.
- **The end's acts, answered and kept.** The forge connector erases an
  act's row when its answer is saved: made (the compound, the account
  removed); refused *ended* for a chat or *unknown* for either, which
  jig answers past its retention for a chat closed or an account
  removed since, both taken as done; or any other saved refusal, named
  in serve's log. It keeps the row on an answer jig does not save: *not
  offered*, the act not granted; lacking the role, no first owner
  configured; or busy, not ready. The first two are named in serve's
  log too, never dropped. At each start of the engine, once the core is
  ready (jig's restart's `Open`), it sends every kept row's act again,
  with its key: an act made before the restart is answered from its
  saved answer, the chat closed once, and an act the restart lost is
  made then, so an end that no client saw, or that a restart cut, ends
  whole.
- **`temper fake forge check DIRS N OBSERVATIONS STORE`,** no step: the
  QA loop's check of the fake forge's promises (`../qa.md`, 8.3),
  skein's judge (skein's `fake-forge.md`, 2.5) over forge N's
  observations, with temper's decisions from temper's own record, a
  copy of its store, never from the merges judged (decision 259). It
  reads the document for forge N, a fake `setup` entry, and for the
  store's limits; it reaches no peer and writes nothing but the copy,
  which opening may repair.
  - *OBSERVATIONS,* the file `--observations` wrote, or a copy of it:
    its lines through its last checkpoint, the flush boundary, whose
    count is the judge's loss. What follows is a flush still being
    written, and is not read. A file without a checkpoint, or a line
    before it that skein's decoder refuses at the fake's limits, is
    undecided: `observations missing`.
  - *STORE,* a copy of the deployment's store, `store/` in
    `engine.deployment.directory`, never the store itself, taken once
    serve has exited, stopped or killed: `/bin/cp -R -- <store>/.
    <empty directory>/`. A copy taken while serve runs is not one:
    skein's KV store has one process (skein's `kv.md`, section 1), and a
    copy made beside its writer can open to part of a commit with
    nothing refused. The lab takes it after the run's processes stop
    (skein's `lab.md`, its collection after the stop), so a run whose
    lab has no such step has no copy. It is opened as serve's store
    opens it, at the limits `temper-service` derives from the document,
    and its format checked as jig checks it, temper's interval read in
    key order; one that is absent, does not open, is of another
    `STORE_VERSION`, or has a row that does not decode is undecided:
    `store unreadable`.
  - *The profile,* temper's as a client of forge N: `client` forge N's
    `Writer`, the bot's number on the fake, and with none, undecided:
    `client unknown`; `marker` temper's key's delimiters,
    `<!-- temper:key ` and ` -->` with its line feed (forge.md,
    section 4); `owned_branches` the `prefix` of each of forge N's
    `Repository` rows; `allowed` the writes temper makes on items it
    did not open: `Comment`, `Merge` and `Close`, and `Review` and
    `Request` when one of those rows' `kinds` grants `review`, since a
    task reviews an outside contribution and sets its reviewers
    (`../domain/forge.md`, section 13), while an update of one stays
    foreign, since temper updates only its own change's branch
    (section 10 there); `bounds` the decisions' count, and
    `temper-fake`'s constant for the objects, past which the judge is
    undecided: `judge full`. The constant is sized for the runs the check
    judges, not for the fake's limits: the judge reserves each repository
    slot's tables at it, so its memory grows with its square, and a run's
    history, each branch and key it saw, is bounded by no limit.
  - *The decisions,* each merge temper decided on forge N, as the
    judge's `Decision`: its repository `owner/name`, its pull and its
    head. They are read from two families (store.md, section 2):
    - each `Entry` whose write is a merge, settled or not, sent or not:
      its `Write::Merge`'s number and head, and its repository;
    - each `Landed` row, a merge temper made, joined by its task to the
      task's `Change` row: that row's repository, `pull` and
      `last_head`, since a change's head stays put from its decision to
      its landing.

    A change in `Landing`, or held over it, adds none of its own: its
    `Entry` stands for it until the merge settles, and its `Landed`
    row after, if temper made the merge. A head is the fake's commit,
    in skein's `fake-forge.md`, section 3's form: `mix(count)`
    big-endian (the SplitMix64 finalizer), the count big-endian, then
    four zero bytes, padded to 32 bytes with zeros in the store. Any
    other is undecided: `store unreadable`. So is a `Landed` row whose
    task has no `Change` row, or a `Change` row with no `pull` or
    `last_head`.
    Rows of other forges' repositories are skipped. The decisions may
    be incomplete when the `Landed` rows number the forge domain's
    `landings` limit (store.md, section 7), or when a decision's
    `Repository` row is gone. An `Order` breach is then undecided,
    `decisions incomplete`, not a fail, since a decision missing can
    only make a merge look undecided; the other breaches stand.
  - *Its order:* the decisions, then `decided`, then the observations,
    then the verdict, so no merge is judged before every decision is
    in. The collection copies OBSERVATIONS no later than STORE: a
    decision is saved before its merge is sent (`../domain/forge.md`,
    section 6), so a store copied after the observations holds the
    decision behind every merge they hold.
  - *Its verdict,* by the check's contract (skein's `lab.md`, 7.2;
    decision 259): a pass is status 0 and no line; breaches are status
    1, a `{"fail": LINE, "summary": TEXT}` line each, its `line()` the
    signature and its summary the promise's words, then an
    `{"undecided": REASON}` line when the evidence is also incomplete;
    undecided alone is status 2 and its line. The judge's reasons read
    `observations lost`, `judge full` and `observations unforwarded`,
    and the check's own `observations missing`, `store unreadable`,
    `client unknown` and `decisions incomplete`; with the decisions
    given first and the stream cut at a checkpoint, no merge precedes
    `decided` and no stream is truncated.
  - *Its stderr* is what it read, kept as its evidence, in JSON lines:
    `{"boundary":{"lines":N,"lost":L}}`, the checkpoint's line count
    and loss; `{"profile":PROFILE,"decisions_complete":BOOL}`, the
    profile's members by their names in skein's `Profile`; and
    `{"decision":{"repository":OWNER/NAME,"number":N,"head":H}}` for
    each decision, its head the fake's commit number. A reader's refusal
    follows as a text line.
  - *Its own errors:* a refused argument, a document that does not read
    or check, or a forge N without a fake entry is the check's error,
    not the run's: status 1, its refusal on stderr and no line, which
    the lab reads as the check's error, never a pass (skein's `lab.md`,
    7.2). The check creates no directory.
- **Its code:** `temper-fake`, step code (section 11): the member's
  types and checks; the fake LLM's and the fake forge's configurations
  and seeds, from the deployment's document or from one of their own;
  `steps`, the box's steps for the document, the demo's group among
  the asked. It names no host. Its dependencies:
  `temper-forge-setup`, `skein-fake-forge-domain`,
  `skein-fake-forge-protocol`, `skein-fake-llm-domain`,
  `skein-fake-llm-protocol`, `skein-llm`, `skein-http`, `skein-forge`,
  `skein-fake-peers`, `jig-protocol-config`, `skein-lib` and
  `skein-json`; and for the check, `temper-protocol-store` and
  `temper-engine-domain-forge`, since the check's derivation is
  `temper-fake`'s too: from the store's rows to the judge's profile
  and decisions, and from its verdict to the check's lines. Its table
  fixtures also name `skein-forge-client`,
  `temper-engine-domain-forge-change` and
  `temper-engine-domain-forge-issues`. The binary's
  `crates/temper/src/fake.rs` hosts the peers on skein-shell, reads the
  check's files and opens the store's copy at the limits
  `temper-service`'s `startup::store_limits` derives from the document,
  and `temper fake git` is `temper-fake-git`'s core on the real file
  system (`../domain/testing.md`, 4.3), which `main` runs when its
  `argv[0]`'s last component is `git`.
  `Mode::Fake` and the link step's kind are `temper-forge-setup`'s.
- **Not yet:** git's smart HTTP on the fake, its pages, and its state
  across starts; and a webhooks' listener in the box, so the demo's
  forge delivers as the QA loop's does; each is the demo's follow-up
  (`../draft/qa-loop.md`, sections 4 and 11).

## 9. Credentials

| Credential | Mode | Made by | Kept | Used by |
|---|---|---|---|---|
| a site admin's token | Forgejo, managed; owned; fake | the person, pasted; the preparation, by Forgejo's command line, or drawn for a fake, or the fake from its seed in a document's form (8.4) | the `AdminToken` record; an owned or fake forge's handover, until the engine takes it, which a fake forge reads at its start | the setup host alone, while a plan runs: reconcile, `TakeOver`, access at adoption (`Adopt`), and the picker's read (`Reachable`, skein's `forge-setup.md`, 4.8) |
| the bot's API and git tokens | Forgejo, every mode | the person, pasted; reconcile | the `ApiToken` and `GitToken` records | the forge component; the identities' table |
| the bot's password | Forgejo, managed, owned, fake | reconcile, random, when it makes the bot | nowhere | nothing |
| temper's admin's password | owned | Forgejo, random | nowhere | nothing |
| the person's first password | owned | Forgejo, random, at the first preparation | `person_password` | the person |
| the runner's secret | owned | the preparation | `runner/secret` | Forgejo's command line; the runner's connection, through the token's file it names |
| Forgejo's own secrets | owned | the preparation | `state`, and `app.ini` | Forgejo |
| the webhooks' secret | Forgejo, every mode | the person, pasted; reconcile, drawn | the `WebhookSecret` record | the webhooks' listener; Forgejo's hooks |
| a GitHub App's key | GitHub | GitHub, at the manifest's conversion; the person, pasted as PEM | the `AppKey` record, DER; a PEM never | the forge component, for its tokens; the setup host, while a plan runs |
| an installation's token; a GitHub git token | GitHub | the forge component, from the App's key | memory alone | the forge component; git |
| the demo model's key | the demo | `temper init`, random (8.2), or the fake LLM from its seed in a document's form (8.4) | `demo/llm.key`, or the document's `key` | the fake LLM, its one credential; the service, which carries it at the demo's start for the deployment's account (8.4) |

- **Every credential the engine holds is a record** in jig's secrets'
  store (credentials.md, the records), written by the setup host's
  plans; a pasted value is kept by jig until its run ends
  (credentials.md, pasted values).
- **Least power, kept.** The forge component holds the bot's tokens and
  the webhooks' secret, or the App's key, as in connected, and never an
  admin's token; the bot is never a site admin; the setup host holds the
  admin's token only while a plan that asked for it runs, and drops it
  at its end.
- **No secret in an argument, a URL or a log,** for Forgejo's command
  line as for git (credentials.md, section 4): 6.3.
- **No new credential to set up from the client:** `temper forge
  fetch` sends none and keeps none, its downloads trusted by their
  pinned digests alone (6.6); the box's pipe carries no secret, a
  failure's reason among its lines (7.1); and the handover is taken as
  at a start (6.5).
- **Never across a domain boundary:** the setup host is in the engine's
  protocol layer, below the core, which sees a pasted value's number and
  digest alone; the engine's rules (credentials.md, section 1) hold for
  the records and for the handover.

## 10. Webhooks on a box

The listener is skein's (`forge-connection.md`, section 6), and so are
the signature, the events and the hints they become (`forge.md`, section
7). This section says what makes them reach the engine on a box, and
that they cover what its work waits on.

- **Made at adoption, checked at each start** (section 5; skein's
  `forge-setup.md`, 4.5, and 4.3, step 6): one hook per adopted
  repository, to `http://<listener>/hooks/<number>`, with the events of
  skein's `forge.md`, section 7, and the forge's secret. An owned
  Forgejo's `app.ini` allows the listener's address (6.2). On a box,
  nothing is left to the person.
- **What a box's work waits on, and what ends each wait,** with the keep
  reading the forge within `hinted` of the hint, which gathers one
  change's hints into one read (`../domain/forge.md`, 5.2):

  | The domain waits on | Done on the forge by | Its delivery | Its hint |
  |---|---|---|---|
  | a landing branch's tip | a merge, a person's push | `push` | the branch moved |
  | a pull request's head, base or state | a push to its head or base; closing, reopening, retargeting or editing it | `push`; `pull_request` | the branch moved; the item |
  | CI on a head | each Actions job ending | the job's end | the commit |
  | a participating object's comments and reviews | a comment, a review, an approval or a rejection | the item's comment or review events | the item |
  | an issue of a live goal | editing, closing, labelling or assigning it | `issues` | the item |
  | drift of a branch temper holds | a push, a deletion, a creation | `push`; `delete`; `create` | the branch moved; the branch |

- **Echoes go up too.** Forgejo delivers temper's own writes. The client
  reads on every hint it holds a resource for, so each costs one read;
  temper, the owner, drops its own echoes by key and by pusher (skein's
  `forge-client.md`, section 4; `../domain/forge.md`, 5.2).
- **What Forgejo settles after it delivers,** a pull request's
  mergeability, which it answers not mergeable while it checks (skein's
  `forge.md`, section 9), is read again at `hinted`, doubling up to
  `poll` apart, within `settle` of the move, as is a hinted job's CI
  that reads pending.
  These are the owner's fresh reads: the client runs no window (skein's
  `forge-client.md`, sections 4 and 5). A pull request that reads not
  mergeable within `settle` of the move is never taken as conflicted
  (`../domain/forge.md`, 5.2).
- **What sends no delivery,** and so is found by polling alone: a
  commit status that a system other than Actions posts, which a
  box has none of.
- **Polling is unchanged** (skein's `forge-client.md`, section 4): the
  keep's alarms, its paces and the slow pass are what they are with no
  webhook. A delivery lost while `temper serve` is down, refused for its
  size, or never sent is found by them; Forgejo does not send a failed
  delivery again (skein's `forge.md`, section 9).
- **GitHub sends no delivery to temper:** its App's changes come from
  the App's delivery log, read by the forge component, with no address
  GitHub must reach and no webhook secret (forge.md, a GitHub App's
  delivery log; 17.6).
- **Checked:** the process tier's hinted story (13.4) checks the
  table's third row, CI on a head: the one wait of its landing that the
  forge ends. Its other waits end on temper's own writes, its push, its
  pull request and its merge, whose deliveries are echoes. Within its
  budget, only the job's end delivery brings a read of its CI:
  - the keep's `poll`, `poll_max` and `slow` are raised past the budget
    (configuration.md, section 7);
  - so is the change's `stall`, whose end reads its pull request afresh
    before it holds the change (`../domain/forge.md`, 5.3 and 8.4);
  - its job outlasts `settle` from its change's push, so the rereads
    within `settle` end before CI does (`../domain/forge.md`, 5.2).

  The first two rows, moved by someone else's push or edit, and the
  others, comments, reviews, issues and drift, are checked from hints
  in the worlds (`../domain/testing.md`, 2.2 and 6.3).

## 11. The crates

```
skein-forge                  + the setup's documents: admin users and their tokens, orgs, repositories'
│                              collaborators and hooks, GitHub's App documents; both sides, as its
│                              others; the pins, the versions and the fetched binaries' digests
│                              (skein's forge.md, sections 4 and 5)
skein-forge-setup            the setup's plans, their types and checks, the access they end with, and the
│                              host component a service runs them on (skein's forge-setup.md, section 6)
├── skein-forge
└── skein-lib, skein-json
temper-forge-setup           step code: the setup's types and checks, read from the configuration; the
│                              setup mapped onto the plans' given values, and the kit's check of them;
│                              the fetch's machine and the preparation's; app.ini and the runner's
│                              files; the processes' specs; the configuration's steps. Names no host
├── skein-forge-setup, skein-forge
└── jig-protocol-config, skein-lib, skein-json
temper-forge-setup-shell     ordinary Rust: the hosts of fetch, prepare and run: commands, files, the
│                              forge's locks, the exec, the fetch's requests over TLS, on skein-io
├── temper-forge-setup
└── skein-io, skein-http, skein-tls, skein-lib, skein-json, skein-shell
temper-box                   the box's machine (step code), its asked groups and its pipe to temper
│                              serve among it, and its host (ordinary Rust) on skein-io's processes;
│                              nothing of setup but its steps (7.3); one crate, under the subset's
│                              lints, its host written within the subset
├── temper-forge-setup
└── skein-lib, skein-io, skein-shell
temper-init                  step code: the default document and the files temper init writes, from
│                              the facts the binary finds (8.2, 8.3); reads, writes and binds nothing
└── skein-lib, skein-json
temper-fake                  step code: the demo's member, its types and checks; the fakes'
│                              configurations and seeds; the box's steps (8.4).
│                              Names no host
├── temper-forge-setup
├── skein-fake-forge-domain, skein-fake-forge-protocol, skein-fake-llm-domain, skein-fake-peers
└── jig-protocol-config, skein-lib, skein-json
temper-fake-git              ordinary Rust: the fake git program's core, temper fake git, its replay and
│                              its two remotes (../domain/testing.md, 4.3)
├── skein-fake-checkout, skein-fake-forge-domain, skein-fake-forge-protocol, skein-fake-peers
└── skein-lib, skein-io, skein-http, skein-json
temper                       the binary: temper forge fetch|prepare|run, temper box, temper init,
                               temper fake llm|forge|git
```

- **The engine runs the setup host** in its protocol layer (5.1):
  temper-service maps each forge's setup onto the plans' given values
  through `temper-forge-setup` (section 5), and depends on neither
  `temper-forge-setup-shell` nor `temper-box`.
- **The binary checks `setup` and its limits** for every command,
  `temper serve`'s included: `temper-forge-setup` checks `setup` and the
  `setup.` limits, and `temper-box` the `box.` limits, beside
  temper-service's startup.
- **Machines, not scripts.** The plans (skein's) and the preparation
  decide from what they read, and say each request or command they need;
  a host runs it and hands back its outcome. Step tests drive them with
  answers; the plans' world, skein's, drives them against the fake forge
  (13.2).
- **Not the throwaway's code.** `testing/skein-throwaway-forgejo` is a
  test crate (skein's `fake-forge.md`, section 6). The production
  machines are written on skein, under the subset, and the process tier
  provisions its Forgejo with them (13.4).
- **The fake forge serves the setup's documents** too
  (`skein-fake-forge-protocol`, skein's `fake-forge.md`, section 4),
  from the server's side of `skein-forge`, as it serves the engine's.
- **The fakes are product,** `crates/skein-fake-*` (skein's
  `fake-forge.md`, section 1), since `temper fake`
  ships them; the worlds and the real loop use them as before.

## 12. Where it serves

- **The first milestone** (`../README.md`, section 2), on a box run from
  a copy of the owned example (8.1), its CI Actions on the owned runner.
  `docs/guide/box.md` is its walk, from the binaries to a change asked
  for.
- **The process tier** (`../domain/testing.md`, 2.6), on owned mode in place of
  its own provisioning, with CI and webhooks (13.4).
- **QA's environments** (`../draft/qa.md`, 4.1), each an owned setup made
  from the same commands (7.4): a box an agent starts, drives through
  temper's client and Forgejo's API as the person, reads the logs in its
  files, and throws away.

## 13. Testing

The tiers, their budgets, the fakes and the layout are `../domain/testing.md`'s;
this section names what checks this document's contract, and what it
relies on.

### 13.1 Step tests

- **Forgejo's and the runner's files:** `app.ini` and the runner's
  configuration for each setup, as golden text.
- **The preparation's decisions:** a first preparation; one at the same
  versions; an upgrade, dumped before migrated; a newer recorded version
  refused; a runner added, under its lock, and one removed; another
  binary refused; a missing `admin_token` made again, under a fresh name;
  a preparation cut short made again from an empty layout; Forgejo's
  files with no `state` refused; a later preparation taking Forgejo's
  lock alone.
- **The specs:** each command's arguments and environment, and no secret
  of the preparation's in any argument.
- **The plans' decisions** from answers are skein's (skein's
  `forge-setup.md`, section 7). What is temper's is the mapping of
  section 5, in temper-forge-setup: each member of a managed setup and
  of an owned one becomes its value, the word `temper` and the names
  `temper-api` and `temper-git` among them, and each `Place` the kit's
  check refuses is named by its path.
- **The configuration:** each mismatch of section 8 refused, naming its
  path; a managed entry with a `directory`, and an owned or fake one
  without, refused; `steps` for each arrangement of modes, with and
  without the demo.
- **The demo,** in temper-fake: each bound of `application.demo`
  refused, naming its path; the fakes' configurations and seeds from
  the default document; `steps` with the demo's group asked and
  unrecorded.
- **The box's machine:** the order; a restart after its once steps; the
  backoff and the cap; the stop's order, and `temper serve`'s second
  signal; a link step started on its long step's first line, given it
  alone, once per box, and given a restart's first line when the long
  step exited before writing one.
- **The fetch's machine** (6.6), in temper-forge-setup, given its URLs
  and digests: each pin's URL and digest for each architecture, and
  another architecture refused; another status or a redirect refused;
  a body past `setup.fetch_bytes` refused. With its host, in
  temper-forge-setup-shell's tests against a local file server over
  TLS, its root skein's test root named by `SSL_CERT_FILE`: a wrong
  digest, its part file removed and its refusal naming the URL and both
  digests; the right one, made 0o755 and renamed; a file already there
  hashed, nothing asked when it matches, and a mismatch refused, naming
  its path, the file left; a part file left by a fetch cut short
  written again.
- **The box's asked groups and its pipe** (7.1), in temper-box: a
  dormant group running nothing at the box's start; `start FORGE`
  running the group's steps in order, a `step` line for each, then the
  record written and `started FORGE`; a step's failure answering `failed
  FORGE WHY`, the group dormant again and the others running on; a
  second `start` while one runs answered busy, a re-ask of the starting
  forge answered by its own lines, an ask while stopping, a record not
  written, a forge with no group `not asked`, and a started one
  `started`; a start with the record
  running the group as an always group; an unrecorded group, the
  demo's, answering `started` with no record written, and dormant again
  at the box's next start until asked; a line past `box.line_bytes`
  refused; `temper serve` restarted with a pipe afresh, an answer meant
  for the exited one dropped.
- **The box's terminal and logs** (7.2), in temper-box, its clock the
  test's: given one pipe for its stdout and its stderr, serve's link
  written in pieces between other steps' lines reads whole there; the
  sign-in block's parts in order around it, and a later link's shorter
  block; a step's lines in its log and not on the terminal, and on both
  with `--echo steps`; no file holding a link; the terminal's lines
  for a group's start, `started`, `failed` and `stopped`, by its label,
  a restart, a give up and a stop; a failure's tail, its lines and its
  log's path; a log moved to `.1` at `box.log_bytes`, and the last run's
  moved to `logs.previous/` at a start; a log not written named once,
  its step's lines then on the terminal.
- **The setup host's handover at runtime** (5.1), in temper-service's
  step tests: at `started`, `TakeOver` when the handover is present, its
  record kept before removal, then `Reconcile` and the connect, the row
  "Connecting" between; a stop or failed removal leaving it "Set up
  failed", named by its step; `failed` naming the box's step and
  pointing to its output for the rest; with no pipe, the row naming the
  commands (6.5).
- **The setup host's close** (5.1), in the existing temper-service
  setup step fixture: notes full and never popped, host outputs full
  before close is requested, and `Closed` behind the active notes,
  store, forge and events gates. The close request is retained, host
  `Close` is sent once when its bounded room permits, and only the
  actual host/socket terminal events and io settlement complete it.
  Events and counts establish the order, with no sleeps, new harness,
  unbounded queue or timeout change.
- **The demo's state and start** (8.4), in the engine's domain world
  (`tests/engine/domain`) and the forge connector's tests: no record,
  not started, the demo read answering it; a start writing running and
  asking for the demo's group in one decision, the box link holding it
  while another group starts; the account added unless the deployment
  holds one of the provider, the value left unused otherwise; the
  demo's project made once, found again by the row and, after a start
  cut short, by its name; the
  adoption once the forge connects, unless the project has it; the
  answer, running, once all three are done, and start failed at the first
  that fails, the forge not connected by `setup.deadline_ms` among them;
  a second start while running answered the same, adding and adopting
  nothing twice; the same key answered from memory, and unknown after a
  restart; a restart while running asking for the group again and
  making no act but an adoption a start left undone, made once the
  forge connects with no client, and once across starts; each act the
  deployment's, the forge connector's, refused not offered when the
  document does not grant it, the start failing *not granted* with the
  `acts` line to add while the engine starts and serves on that
  document; refused,
  not offered, with no demo or one ended; and in temper-service's step
  tests, a start carrying `llm.key`'s bytes.
- **The demo's end's acts** (8.4), in the engine's domain world: the
  selection both ways, a chat of the demo's project open at the end
  closed with its delegates and the reason, and a person's chat opened
  after the end in the same project, on their first repository, left
  open; the demo model's account removed, and none asked without one;
  each act kept in the end's decision and its row erased on its saved
  answer, ended and unknown among them; a restart between the keep and
  the answer sending it again, answered once, the chat closed once; an
  act not granted kept, named in the log, and sent again at the next
  start.
- **The fake's start-time adoption** (8.4), in temper-service's step
  tests: after handover and reconcile, one `Adopt` for each seeded
  repository not already restored, in order, with the deployment's
  listener when configured;
  the admin token and webhooks' secret answered from records, and the
  connect's access built from records; a stop named by its step.
  `temper-init`'s step tests check the document's listener, and the
  binary's crate tests the five ports distinct.

### 13.2 Reconcile's world

The world is skein's (`forge-setup.md`, section 7): the plans on the
fake forge's protocol layer, serving the setup's routes, and the host
component's own waits and bounds. What is temper's is the engine's
routing of the setup host, checked in `temper-service`'s own tests
(`forge.md`, section 10) and by the process tier (13.4):

- a start's handover taken, its file removed only after `TakeOver`'s
  `Done`, and left when it stops; a handover readable by others
  refused, naming it;
- each write answered `Written` only once kept, so a restart between
  finds what a browser was told;
- a read served from the run's pasted value before the forge's record;
  a pasted value whose digest differs refused, "the pasted value
  changed";
- one writing plan per forge and one `Reachable` beside it, a second of
  either refused busy, and a request past `setup.runs` refused busy;
- a start's reconcile that stops leaving that forge not connected,
  named with its step and no secret, and the others served;
- records made for another kind or address leaving the forge not
  connected, named, nothing erased; an `api_token`, `git_token`, `user`
  or `webhook_secret` file in a forge's directory left unread.

### 13.3 What the setup relies on

Forgejo's and the runner's behaviours at the pinned versions (6.1), each
recorded by skein's conformance (`fake-forge.md`, section 7) on the
throwaway Forgejo and runner, and named here by its fact in the
conformance's `facts.txt`. What reconcile relies on, and the deliveries,
are skein's: `forge-setup.md`, section 8 and `forge.md`, section 9. What
the preparation relies on stays here:

- **Forgejo's command line** (6.3):
  - `admin user create --random-password` prints `generated random
    password is '<password>'` on its first line
    (`cli-user-create-prints-the-random-password-first`), and refuses a
    user made twice (`cli-user-create-again-is-refused-naming-the-user`);
  - `admin user generate-access-token --raw --scopes` prints the token
    alone (`cli-token-raw-prints-the-token-alone`), limited to the scopes
    it names, comma-joined (`cli-token-scopes-limit-what-the-token-reads`,
    `cli-token-admin-scopes-comma-joined-are-taken-under-two-suffixes`),
    and refuses a second token of one name
    (`cli-token-name-used-twice-is-refused`);
  - `migrate` runs on a fresh work path, and again on a migrated one
    (`cli-migrate-runs-on-a-fresh-work-path`,
    `cli-migrate-runs-again-on-a-migrated-work-path`);
  - `dump` needs the work path's repositories directory and its
    `--tempdir` (`cli-dump-needs-the-repositories-directory`,
    `cli-dump-needs-its-tempdir-to-exist`), adds `.zip` to a name without
    it (`cli-dump-appends-zip-to-a-file-name-without-it`), and overwrites
    a file of its name (`cli-dump-over-an-existing-file-overwrites-it`);
  - a user made to change its password is refused the API until it has
    (`cli-user-made-to-change-its-password-is-refused-the-api-until-cleared`).
- **The runner** (6.3):
  - `forgejo-cli actions register --secret-file` takes exactly 40
    hexadecimal digits with no line feed
    (`runner-register-refuses-a-secret-of-another-length`,
    `runner-register-refuses-a-secret-that-is-not-hexadecimal`), and run
    again with the same secret keeps one global runner
    (`runner-register-again-keeps-one-global-runner`);
  - the runner's `create-runner-file` takes its secret only as an
    argument
    (`runner-create-runner-file-takes-its-secret-only-as-an-argument`);
  - a connection of the runner's `server.connections` has, as its `uuid`,
    the secret's first sixteen characters in hexadecimal, and as its
    token the secret
    (`runner-uuid-is-the-secrets-first-sixteen-characters-in-hex`,
    `runner-token-is-the-secret`), which a file may hold (`token_url:
    file:`; `runner-connection-with-a-token-file-comes-online`); a runner
    started on that connection alone takes the instance's jobs
    (`runner-connection-alone-takes-the-jobs`).

### 13.4 The process tier

The tier, its budget and its referee are `../domain/testing.md`'s (2.6). What
it checks of this document:

- **Its directories:** every command a test runs is given the three
  directories' flags, each a scratch directory of the test's own; the
  owned example is copied as a configuration directory (8.1). No test
  resolves a default under `$HOME`.
- **Owned in place of the test's support:** the test runs `temper forge
  prepare`, `run forgejo` and `run runner` itself, then `temper serve`,
  whose setup host takes the handover and reconciles (6.5), with the
  binaries `TEMPER_FORGEJO` and `TEMPER_FORGEJO_RUNNER` name, and
  webhooks on. Its repository runs Actions, so its landing rule asks for
  CI.
- **The hinted story:** the same story with the keep's `poll`,
  `poll_max` and `slow` and the change's `stall` raised past the story's
  budget (configuration.md, section 7; `../domain/forge.md`, 8.4), and its
  CI's job outlasting `settle` from the change's push, so that it lands
  within its budget only if the job's end is delivered and the reads
  that follow it (section 10, "Checked"); and with no conflict resolved,
  since its changes never conflict. With no webhooks, the same story
  misses its budget.
- **`temper box`,** a test of its own on a scratch directory: the sign-in
  link on its stdout, and whole on one pipe given as both its stdout and
  its stderr, as a terminal is (7.2); Forgejo killed and restarted, with the engine still
  serving; a stop leaving no process it started.
- **A first run,** a test of its own: `temper box` on three empty
  scratch directories writes the configuration (8.2), says so (init's
  line for no browser among its lines only from a binary built without
  one: the test checks the build it runs in, `../client/browser.md`,
  2.6), and prints one sign-in link, which signs the owner in, the demo's fakes
  dormant; the owner starts the demo with `temper ctl demo-start`
  (`../client/tools.md`, 5.5), answered once the demo forge is
  connected and `playground/widget` adopted into Playground, project 1,
  which the start made. The owner sends
  "Add CSV export to widget" to the project's chat, which runs on the
  demo model with no pick (8.3); the chat's last message says the
  change landed, its delegated task ended landed, and the landed
  change's merge commit, the id temper's own read of the change gives,
  holds on the fake the seeded head as its first parent and the seeded
  files with `src/main.rs` the coding agent's edit (8.4): the test reads
  the commit's tree, file bytes and parents by the fake's `POST
  /.fake/git/objects` (skein's `fake-forge.md`), which takes no token,
  so the story reads no credential. The test prints each box run's
  time, asserting no time. It reads no hook, since the fake's hook
  read takes a token. A delivery's acceptance is shown by
  `tests/real-loop/tests/forge.rs`'s
  `a_forge_delivery_is_accepted_by_the_engines_listener`; its secret's
  matching is checked by the step tests. A second box on the same
  directories finds the demo running, starts its fakes at serve's ask
  with no act made again, and its clone holds the seeded head, since
  the fake keeps no state across its starts.
- **The playground's end,** a test of its own: a first run's box, the
  demo started and connected, then a second box on the same
  directories, its document given a second fake forge, which runs in
  its place, unasked, and connects at serve's start: the demo ends
  (8.4), the box's terminal says `Playground: stopped` (7.2),
  `wait forges` lists the second forge alone, and `temper ctl
  demo-start` is answered not offered. A third box starts nothing of
  the demo's group, and answers the same. Before the end, a chat is
  opened on the demo, and no client watches the end: once the third
  box is up, the chat reads cancelled by the deployment, `Ended with
  the playground`, and the deployment holds no account of the demo's
  provider.
- **Two first runs, one after the other,** a test of its own: a first
  run's box, signed in, then a second box on three other empty scratch
  directories while the first runs. The second's init finds the first's
  ports held and writes the ports the kernel assigns (8.2), and the
  second signs in, starts its demo and answers a suggested request on
  its own fakes while the first, which starts no demo, holds the
  defaults. No first-run story takes a lock
  on the default ports: each box holds its own (7.1), so the stories
  run beside each other as any others do.
- **A start a restart cut short,** a test of its own: a first run's
  box stopped once the demo's start has written running and before its
  forge connects; a second box on the same directories, with no client
  and no start sent, adopts `playground/widget` into the demo's project
  once the forge connects, and a chat on the demo then answers.
- **A first run set up for real,** a test of its own: `temper box` on
  three empty scratch directories, with the binaries `TEMPER_FORGEJO`
  and `TEMPER_FORGEJO_RUNNER` name placed where the fetch looks,
  `<cache>/forgejo/VERSION/forgejo` and
  `<cache>/forgejo-runner/RUNNER_VERSION/forgejo-runner`, so that the
  fetch hashes them against the pins and fetches nothing (6.6): no
  mirror, no TLS and no call off the machine. The owner asks "Set up a
  Forgejo here" through the client, `temper ctl DIRECTORY forge-setup
  3`, answered setting up; forge 3's row, followed by `wait forges
  --until settled`, goes through "Setting up" and "Connecting" to
  "Connected as temper", with no restart, and its first repository made,
  `work/hello`; the call adopts it into the new project `hello`,
  project 2, after the demo's project 1 (6.7). The
  workflow's push has run on the owned runner: before the owner's first
  task, the runner's output, which the box gives on its stderr as
  `runner 3: ` lines, says it took a task on `work/hello`, main's
  push's, since no pull exists yet. It shows the run taken, not its
  verdict: no read of temper's says CI at a repository's `main`, and
  the story reads no credential. The owner's first task
  in the project, on the fake LLM, changes `hello`: its worker clones
  it, its pull's run of the starter workflow passes, and it lands; the
  test reads it all through temper's own reads, with no credential:
  the set up's answer names `work/hello` adopted, owned, on forge 3;
  the task, given in project 2, runs on and lands into `work/hello`;
  the change's checks at its pull's head are the starter workflow's
  run, passed; and the task ends landed. The
  handover file is gone, and
  forge 3's directory has no `api_token`, `git_token`, `user` or
  `webhook_secret` file. The service's step world shows the git value
  and its generation (forge.md, section 10, the box's link). A second
  box on the same directories starts the group by its record, before
  `temper serve`. Preparation makes a new handover; serve takes it over
  and connects forge 3 from its records (6.5), and the task is found
  landed, its CI at its head unknown until a next step reads it
  (`../domain/forge.md`, 8.8).
- **The first run whole,** a test of its own: one box on three empty
  scratch directories, forge 3's binaries placed where its fetch looks,
  with no restart. The person signs in and starts the playground
  (`temper ctl DIRECTORY demo-start`), and its chat in project 1 is
  answered by the scripted model. Then "Set up a Forgejo here",
  `forge-setup 3` and `wait forges --until settled`: forge 3 reads
  "Connected as temper", `work/hello` made and adopted into the new
  project `hello`, project 2. That connect ends the playground at
  runtime (8.4): the box's stderr says `box: forge 1's group stopped`
  once, no list shows forge 1, the playground's chat reads cancelled by
  the deployment, `Ended with the playground`, the deployment holds no
  account of the demo's provider, and `demo-start` is answered not
  offered. The owner's first task in project 2 then lands on
  `work/hello`, read as in the test above. On Linux, Forgejo's step is
  given the listener the box held from its start (6.4, 7.1).

## 15. Later

Each is intended; what it needs is named with it.

- **After the first repository** (6.7): "New repository" on a connected owned or managed card is the general
  form of the same flow, with the same fields and `Create`, a managed
  forge's org needing an owner. A new project, named in the form,
  waits on jig's projects at runtime (MakeProject); until jig carries
  a follow-up act on `SetUp`'s admission, the first repository's
  adoption is the client's, while the demo's start makes its acts as
  the deployment's (8.4). Projects are
  seeded at the first start, and made at runtime by jig's
  `MakeProject` (jig's `domain/core.md`, 6.1): the first repository's
  set up makes its project, named after the repository unless the
  person names one, so the first run's `work/hello` is project 2's,
  after the seeded project 1 (`../client/tools.md`, 5.5;
  `../draft/first-run.md`, 2.4). The playground's start still adopts
  into the seeded project 1 until its own project (below). The set up's intent is kept in
  memory alone; keeping it across a restart is open (section 16).
- **The playground's own project** (8.4): once jig's deployment acts
  make a project (`MakeProject`, jig's `domain/people.md`, 5.3), the
  demo's start makes Playground as its first act, and the demo's chats
  become simply the open chats of that project, read from it at the
  end in place of the end's selection by the moment (8.4, step 5),
  with `make_project` in the document's grant (8.3).
- **A lock and an exec in skein's shell,** onto which 6.4's module
  moves once skein's shell offers an exec and a lock held across it,
  which skein's `Lock` is not (skein's `io.md`, section 5). A second
  service that execs is what would ask skein for them.
- **A story of Reconcile over TLS.** The managed string form takes a
  forge on another host over TLS and the service hands that transport to
  skein (5.6), but every test of the admin connect, the process story
  among them, reaches its Forgejo over loopback plaintext: a TLS story
  waits for a fake forge that serves TLS (skein's `forge-setup.md`,
  section 9).
- **Webhooks from another host,** over TLS (skein's
  `forge-connection.md`, section 6), once skein has TLS's server side.
- **CI's isolation.** An owned runner's jobs in host mode run as the
  box's user, who can read every file of temper's as credentials.md,
  section 8 accepts of agents in standalone, the handover among them
  until the engine takes it. Until they are isolated, an owned forge
  runs the person's and temper's agents' code only, never a stranger's
  pull request.
- **Provisioning at adoption** (jig's `domain/connectors.md`, 12): a
  repository in a person's own namespace, made without the person.
- **The forge as people's sign-in provider** (credentials.md, section
  8), which would link the person's account on an owned Forgejo to their
  owner in jig.

## 16. Open questions

- **Good defaults for a person's real work:** charters, landing rules,
  limits, the memory bound. Default: 8.3's working minimum, refined once
  a first run works.
- **Overrides of `app.ini`.** Default: none; the settings are temper's.
- **A token for the person's scripts and agents.** Default: none; the
  person makes one on Forgejo's web, with the scopes they choose. A QA
  harness playing the person makes one so, with read scopes alone, for
  the agent it runs (`../client/tools.md`, section 10); never the
  preparation's `admin_token`, which the engine's setup host alone takes
  (section 9).
- **The set up's intent across a restart** (6.7). Default: in memory
  alone, so a restart before the first repository is made loses it,
  and "New repository" makes it; a kept intent would be a stored
  format to version, which nothing on the first run needs.
- **Backups beyond an upgrade's.** Default: the person's, of the forge's
  directory and the store, with the box stopped.
- **Two deployments on one managed forge.** Default: each its own bot
  login, and so its own collaborators and hooks; the marker does not
  tell deployments apart.
- **A hook left at an old listener's address,** after the listener
  moves, is skein's question (skein's `forge-setup.md`, section 9).
  Default: left alone; the person deletes it.
- **A managed forge whose host list refuses the listener.** Default:
  nothing tells the person but the hints that never arrive
  (`../domain/forge.md`, section 4).
- **The managed picker's departure.** On a managed or owned Forgejo the
  picker lists every repository the site admin can see, not those the
  bot can write, as a connected forge's does, since adoption grants the
  bot its access (5.4). Default: the admin's view, through `Reachable`,
  plus "not listed" by owner and name; the admin's token stays the
  setup host's, and the picker gets the rows alone.
- **"Connect as its admin" on a Forgejo with no `setup` entry.**
  Default: refused, naming `application.setup`, since the bot's login
  and email are the configuration's (5.5).
- **A start's reconcile that stops.** Default: that forge is not
  connected, named with its step, until the next start, a managed
  connect or an owned forge's next set up ask (6.5).
- **A managed or owned forge stopped at runtime,** its bot's token
  revoked. Default: stopped, named, until the next start's reconcile or
  a managed connect; Replace is a connected forge's alone.
- **The owned example's agent, sized for a person's work.** Default: the
  process tier's profile (8.1), sized for the live tier's stories.
- **The owned example's `memory_bytes`** are at least `small`'s worst
  cases with its strings at their bounds (8.1), with 10% headroom rounded
  up to two significant digits and a checked band through 1.25 times
  each worst case. The exact terms live in
  `crates/temper/tests/figures/memory/owned/{engine,worker}/`.
  They remain more than any machine has, so the start's check bounds
  nothing a person's machine holds.
  Default: as the example states them; which terms of the worst cases
  make them so large is open (configuration.md, section 11).
- **The check's decisions** (8.4) are read from rows kept for other
  ends, and two decisions leave none: a change whose pull is replaced
  while `Landing` joins its `Landed` row to the later pull, and an
  enqueue the forge's merge queue drops leaves no `Entry` once settled,
  a GitHub path the Forgejo fake never takes. Default: as 8.4 reads
  them; a family of temper's decisions, `(repository, pull, head)`
  written in the merge's `Entry`'s commit, is the route when a run
  needs either, a store schema change in place until the first release
  (store.md, section 4; skein's `codec.md`, section 5).

## 17. Connecting GitHub

GitHub is managed or connected, never owned. Its managed mode means an App
made through the manifest, not Forgejo's string-managed admin connect (5.5).
Both GitHub modes end in the same
records (section 3), and the engine runs both alike after; they differ
in who made the App. The plans are skein's (`forge-setup.md`, section
5), run by the setup host (5.1); the browser's routes are the clients'
listener's (`../client/forge.md`, the browser's steps).

### 17.1 GitHub's App and its permissions

- **One App per deployment,** public, so that it installs on any
  account (skein's `forge-setup.md`, section 5). Its permissions:
  contents, pull requests, issues and merge queues, write; checks,
  statuses, actions, administration and metadata, read; never
  workflows, never administration's write. Its events are skein's
  (5.6).
- **Its hook** points at `https://<slug>.invalid/`, an address that
  never resolves, so GitHub delivers nowhere and holds no secret of
  temper's; the App's changes come from its delivery log (17.6).
- **Its records:** `AppId`, `AppSlug`, `AppKey` (DER; a PEM is never
  kept), `User` (the App's bot user), `Accepted` and `Origin`
  (credentials.md, the records).
- **Its addresses** are github.com's when the forge's `api`, `git` and
  `web` are null (configuration.md, section 3); a forge names them only
  to point at a fake.
- **The clients' listener at a fixed address,** a loopback port the
  person's browser reaches, since GitHub's redirect and setup URLs name
  it; its port 0 is refused at start while a GitHub forge is
  configured.
- **A repository's own rules** are read and said at adoption, and never
  changed (`../domain/forge.md`, what adoption says of the branch's
  rules): a merge queue is taken as it is; required approvals need the
  App on the rule's bypass list or a person's approval; required
  signatures, linear history and code owners are each named with the
  one setting that lets temper land.

### 17.2 Managed: the manifest flow

1. **Connect,** "Connect GitHub" on the deployment's page or its tool
   of `temper ctl` (`Connect` with `GitHubManifest { back? }`). The
   setup host runs `Manifest { forge, target }` (skein's
   `forge-setup.md`, 5.1), `target` always `Personal`, for a fresh
   state, ending `Done(Manifest { state, target, manifest })`; temper
   renders the page from them (client/forge.md, 9.1) and holds it and
   `back` with the state. With `back`, the link is a path on the
   page's own origin, `/forges/<n>/github/manifest?state=<s>`: the
   browser goes to it in the page's own tab, as a sign-in's does,
   and comes back to `back` at the end. Without `back`, the link is
   the whole address on the clients' listener,
   `http://<listener>/forges/<n>/github/manifest?state=<s>`; the
   terminal prints it and never opens it.
2. **The manifest's page** posts the manifest to
   `<web>/settings/apps/new?state=<s>` as it loads, by one inline
   script its policy allows by hash, and shows its button where the
   script doesn't run or the page was reached through the browser's
   history; its headers allow a form to `<web>` alone, keep no copy
   and send no referrer.
3. **GitHub's form:** the person creates the App, under their own
   account. Nothing asks for an organization: the App is public, so
   GitHub's install page offers every account the person may install
   it on (with the user, 2026-10-23; an App an organization owns is
   for later, `../draft/github.md`, section 18).
4. **The redirect back,** `http://<listener>/forges/<n>/github/redirect`:
   `Convert { forge, code, state }` (5.2) exchanges the code for the
   App's id, slug and key, finds its bot user, and writes the records;
   the conversion's PEM, webhook secret and OAuth pair are never kept.
   Once they are kept, `Install` (5.4) answers the App's install link,
   the browser goes on to it, and the forge connects (forge.md, the
   setup's host).
5. **Installing:** the person picks an account, and all repositories,
   GitHub's default, or those they select. GitHub sends the browser to
   `http://<listener>/forges/<n>/github/setup`, where `Accept` (5.4)
   checks the installation and adds it to `Accepted`, and the route
   sends the browser back to `back`, or, with none, says "temper has
   the installation; you may close this tab." A member of an
   organization who isn't its owner can only request the App there:
   GitHub comes back with `setup_action=request` and no installation,
   and the route says why temper can't go on, and to install it on
   their own account or an organization they own (client/forge.md,
   9.1).

- **States** are one use, drawn by the setup host, at most
  `setup.states`, each living `setup.state_life_ms`, at most the
  conversion code's hour; a used, unknown or expired one is refused,
  "This link has ended. Start again from temper.", and a restart
  forgets them all.
- **The redirect and setup URLs** temper gives the plans are whole,
  built from the clients' listener's address (forge.md, the setup's
  host). The manifest's `url`, the App's homepage, is
  `http://<listener>/`.

### 17.3 Connected: an App pasted

- **The person makes an App** on GitHub with 17.1's permissions and
  events, and pastes its id and its private key as GitHub gives it,
  PEM: "Connect an App of your own" (`Connect` with `GitHubApp { id, key
  }`). No webhook secret is asked: the paste check sets the App's hook.
- **Its setup URL is the person's to give** on GitHub's form, since no
  plan can set it: `http://<listener>/forges/<n>/github/setup`, with
  redirect on update, no callback URL and no user's authorization at
  install (`docs/guide/github.md`). An installation of an App without
  it never comes back through the setup route, and is a stray (17.4).
- **The paste check,** `Paste { forge, app }` (skein's
  `forge-setup.md`, 5.8): the key, kept by jig until its run ends,
  decoded to DER; the App read with it, its id the one pasted, else
  `OtherApp`; its bot user found; its hook set to
  `https://<slug>.invalid/`; its permissions and events checked as its
  reconcile checks them. It writes `AppId`, `AppSlug`, `AppKey` and
  `User`, the records the manifest flow writes, and ends done, or
  `Lacking` with each permission or event named, never refused for it.
  A forge whose `app_read_only` is true has its hook read and left
  (17.7).
- **A restart during the paste check** drops its run: the carried key
  is erased at start, and the forge reads as its records say
  (`credentials.md`, 2.2; a stored connect, which would say
  "interrupted; paste again", is an open question there).
- **Installing** is 17.4's.
- **Replace,** a new key: the paste check with the kept id, so a key of
  another App stops it `OtherApp` and the forge stays as it was.

### 17.4 Another account, and repositories later

- **"Install on another account"** (`Install { forge, back? }`,
  skein's `forge-setup.md`, 5.4): a fresh state and GitHub's install
  link (`Link`), where the browser goes as at 17.2; the person installs
  on another account or org, and the setup route accepts it and sends
  the browser back.
- **Repositories added later** to an installation of selected
  repositories are the person's, on the installation's page on GitHub;
  GitHub's visit to the setup route with no state refreshes an accepted
  installation's repositories and sends the browser to Forges, and is
  refused otherwise. The forge
  component reads an installation's repositories again on its events.
- **An installation temper did not accept,** a stray made with no
  state of temper's, is shown, and uninstalled by the next reconcile
  once older than a state's life; one deleted on GitHub leaves
  `Accepted`; one suspended is shown.

### 17.5 Its reconcile

- **`Reconcile { forge }`** (skein's `forge-setup.md`, 5.6) runs after
  each `Connected`: the App's key, permissions, events and installations
  checked. It repairs only its hook's address, and with `app_read_only`
  (17.7) nothing: the hook is left and a stray is never uninstalled.
- **A key GitHub refuses** stops the forge: the forge component, which
  signs with the same key, stops it (forge.md, 2.3), named, with
  Replace (17.3); the reconcile's own `Stop` is named to the root and
  changes nothing.
- **A permission or event lacking** is named, `Lacking`, never refused:
  the person grants it in the App's settings and approves it on each
  installation, as GitHub asks.

### 17.6 Job logs, and the delivery log

- **A job's log** lies behind a redirect to a storage host, which the
  forge component follows only to an origin its `log_origins` lists
  (configuration.md, section 3; forge.md, GitHub's API as temper uses
  it): on GitHub one origin by default; `[]` fetches no job log. An
  origin not listed is named on the deployment's page, and the person
  adds it to the forge's `log_origins` and restarts.
- **The delivery log** is read by the forge component every
  `log_poll`, and its deliveries become hints as Forgejo's webhooks do
  (forge.md, a GitHub App's delivery log).

### 17.7 A pasted App another owns: read-only

- **A forge's `app_read_only`** (configuration.md, section 3) says the
  pasted App (17.3), its hook and its installations are another's to
  change: a probe App shared with skein's conformance, whose hook stays
  where its owner put it, as the live tier's (`../domain/testing.md`,
  2.7). The setup host gives it to skein's GitHub plans as their given
  `read_only` (skein's `forge-setup.md`, GitHub's plans).
- **What changes:** the paste check (17.3) and reconcile (17.5) read
  the App's hook and leave it; reconcile lists the installations and
  keeps `Accepted` as it says, and a stray (17.4) is shown and never
  uninstalled.
- **What doesn't:** the App's key, id, permissions and events are read
  and checked, a lacking one named; `Install`'s link and the setup
  route's `Accept` (17.4), which read; and the forge component's
  connection, its mints and its delivery log (17.6).
