# A person's first run

Draft, 2026-10-20. What a person sees the first time they run temper,
and the general pieces it is made of. It records a design session with
the user on 2026-10-20, which started from the assessment of where the
first run stands once the accounts and GitHub work land
(`~/src/rust/tmp/temper-first-time-ux-2026-10-20.md`). It crosses skein
(the arguments' rule, the fakes as product crates) and temper (its
directories, `temper init`, the box, the client) on purpose; an owner
splits it. It builds on `draft/github.md` as it lands: forges connected
from the client, credentials taken at runtime and the forges' records
in jig's secrets' store. Amended with the user on 2026-10-23: the
welcome, and the demo's end (2.3); the box's terminal (2.1, 2.5).

The first run is sugar. Every piece below has a place of its own in the
layering and is useful without the first run; the first run only
arranges them. Nothing here shapes a piece for the first run's sake.

## 1. In one page

- **One command, no argument:** `temper box`. With an empty
  configuration directory it writes a default document there, starts
  what that document names, and prints one link (section 2).
- **The link signs the person in** and opens a welcome: try the demo,
  or set temper up (2.3). The demo is a chat in a project of its own,
  Playground, on a demo repository, with a demo model and a demo forge:
  temper's fakes, which start only when the person chooses the demo
  (sections 3.5, 3.6, 2.3).
- **No project until the person makes one** (2.4): the playground's is
  made when it is chosen, and the first real one with its first
  repository, named after it until the person changes it.
- **Real backends replace the demo** (2.3, amending 2.2): the person's
  first usable model ends the demo model, and their first connected
  forge ends the demo forge, its open chats and its repository's place
  in the project. What the demo did stays readable.
- **Three directories,** XDG's: configuration, data and cache, each a
  flag, an environment variable and a default, in that order of
  precedence (3.1, 3.2).
- **`temper init`** writes the default document, a command of its own,
  which answers setup.md's open question of a command that writes the
  document (3.3).
- **GitHub and an owned Forgejo are in the default document,** so
  connecting either needs no restart. An owned Forgejo there is dormant
  until the person asks for it; then temper fetches its pinned binaries
  into the cache and starts it (3.7, 3.8). A Forgejo elsewhere is at an
  address no default knows, and so is a restart (section 4).
- **The name stays `temper box`,** provisional. The binary's commands as
  a whole (engine, worker, standalone, the box) want a session of their
  own (section 8).

## 2. The person's way

### 2.1 The first run

```
$ temper box
First run: wrote the configuration, ~/.config/temper/temper.json
Logs: ~/.local/share/temper/box/logs/
Starting serve

Open this link in a browser on this machine. It signs you in as this
temper's owner, once and for a short while, so keep it private.
Opened again once spent, it prints a fresh one here.

http://127.0.0.1:8367/jig/v1/sign-in/link?secret=FefMHTruDiW3sTE149SuCHJtUvH8GIulzGpv6X3-CPs

Ctrl-C stops temper.

```

Then, as the person goes (2.3), one line each:

```
14:03:20Z Playground: starting
14:03:21Z Playground: started
14:09:41Z Forge 3 (Forgejo): starting
14:10:02Z Forge 3 (Forgejo): failed, prepare 3 exited 1
    prepare 3: app.ini: Permission denied
    Log: ~/.local/share/temper/box/logs/prepare-3.log
^C
14:31:05Z Stopping
14:31:07Z Stopped
```

A later run on the same directories prints the same without its first
line. What the terminal shows, and why, is 2.5.

1. `temper box` finds `$XDG_CONFIG_HOME/temper` empty, runs `temper
   init` on it (3.3), and starts the document's steps (3.9).
2. The person opens the link in a browser on the same machine. It signs
   them in as the deployment's owner (`../client/README.md`, section 7)
   and lands on the default project's chat.
3. The project has the demo repository adopted, on the demo forge, and
   the demo model as its account (3.6). A banner says it is a demo and
   offers to connect a real model and a real forge.
4. The person tries a suggested request; the demo model plays its
   script and the change goes through temper's procedure on the demo
   forge. A link to the forge's pull request opens a page the demo
   forge does not serve, for now (section 7).

The mockup of what the person sees is `../client/mockups/first-run.html`.

### 2.2 Going real

*Amended by 2.3: the demo ends as real backends arrive; the bullets
below that keep it are superseded.*

- **A model.** The accounts page, as the accounts work leaves it:
  Continue with ChatGPT, use a Claude subscription, or an API key. The
  new account becomes the deployment's default; the demo model stays an
  account and stays the demo project's.
- **A forge.** The Forges page, as `draft/github.md` leaves it: Connect
  GitHub (managed, by the manifest flow, or connected, an App pasted),
  Set up a Forgejo here (owned, 3.8), or a Forgejo elsewhere (managed
  or connected, section 4). A forge connected, the person adopts a
  repository into a new project, or, past the first, into one of
  theirs (2.4). A new
  project's form suggests the repository's name as the project's,
  made unique ("tally 2" beside a "tally"), for the person to change;
  the name is then the project's own, not the repository's (jig's
  `domain/core.md`, 6.1). jig makes no project after a first start
  yet (2.4).
- **The demo** can be hidden; its forge and model stay in the document,
  so a demo is always there to show someone.

### 2.3 The welcome, and the demo's end

With the user, 2026-10-23. Mockups: `../client/mockups/welcome.html`,
in the first-run world (its step 0, the demo chosen at step 1, its end
at step 7) and the first-run-real world (the wizard to a connected
Forgejo and model).

- **The link opens a welcome,** in the browser: "Try the playground"
  or "Set up temper". The terminal asks nothing, since the box also runs
  unattended and going real needs the browser. A document whose
  `application.demo` is null has no demo, and its welcome no demo card.
- **Set up temper is a wizard:** Welcome, Forge, Model, Ready. Forge is
  the client's "Connect a forge" flow; Model is the accounts' ways the
  document offers; the forge comes first, so an owned forge's set-up
  runs while the person adds a model. Ready ends on the new project.
  Each step offers Later, and "Try the playground" stays offered until
  a forge of the person's connects.
- **The demo starts only when chosen:** adopted (first-run-ux's M3),
  its state in `../protocol/setup.md`, 8.4, and `../protocol/store.md`,
  section 2, the box's asked demo group in setup.md 7.1, and the
  client's start in `../client/README.md`, 3.5, 4.6, 4.8 and section 8.
  The seed (3.6) and its link step go: serve's first link is the
  person's.
- **The demo ends for good:** adopted (first-run-ux's M4). The end,
  its record and a restart after it in `../protocol/setup.md`, 8.4,
  the box's `stop` and `stopped` in 7.1 and 7.2; the forge and its
  repository ended in `../domain/forge.md`, 4.1, and
  `../protocol/forge.md`, 2.8 and 8.1; the client's re-read and the
  demo model's account in `../client/README.md`, 4.8. The demo's open
  chats' end and its model's account removed with no client: adopted
  (first-run-ux-2's M3), in setup.md, 8.4, on jig's deployment acts.
- **The person meets the demo as the playground.** "Demo" undersells
  it and "fake" is harsh; the design keeps the demo as its concept's
  name (`application.demo`, the `fake` mode, the demo's halves). The
  words: the welcome's card "Try the playground", "A built-in scripted
  model and forge. Ask one of three requests and follow its change from
  a branch to main."; the banner "This is the playground: a scripted
  model and a playground forge."; the names Scripted model and
  Playground forge (the client's phrasebook, `../client/README.md`,
  5.6), and the repository `playground/widget`, the fake's owner
  `playground`. setup.md 8.4 holds the scripts' words; the client's
  README, its pages (3.5: `Welcome`, `ConnectForge`), the setup strip
  (5.1, 5.6) and the demo's halves ending for good (4.8); its
  `forge.md`, 5.4, the Forges page and the flow.

### 2.4 Projects, made as the person goes

With the user, 2026-10-23. Amends 1, 2.2, 2.3 and 3.4. Mockups:
`../client/mockups/welcome.html`, in the three first-run worlds.

- **Why there was a default project.** jig made projects only at a
  deployment's first start, from its configuration, so the default
  document seeded project 1, "Default", for the playground's repository
  and the owned forge's first one to go into. Nothing of the person's
  was in it, and the set up's form offered it beside "A new project", a
  choice a newcomer cannot make sense of.
- **No project at the first start.** The welcome opens on a deployment
  with none. The default document configures one policy instead, jig's
  `engine.policies` 1, which each project made starts from, and its
  landing rule among the deployment's, so that every project takes it
  (`../protocol/setup.md`, 8.3).
- **The playground's project, "Playground",** is made by the demo's
  start, when the person chooses "Try the playground"
  (`../protocol/setup.md`, 8.4). It keeps the playground's chats, and
  what they did, once the demo ends.
- **The person's first project** is made with their first repository,
  named after it (`hello`, `widget`) until they change it: in the set
  up's form of a Forgejo here, in GitHub's Ready
  (`../client/forge.md`, 5.4), and in the adoption form for a forge
  connected with no repository (`../client/README.md`, 4.4). The first
  run offers no existing project; a later repository, from Forges, goes
  into one of the person's projects or a new one.
- **The playground tried first, then set up:** two projects,
  Playground, its chats ended and readable, and the person's beside it.
- **Waits on jig:** its projects made at runtime (jig's `MakeProject`,
  its `domain/core.md`, 6.1, and its `STATUS.md`, jig-projects,
  designed 2026-10-23) and projects' names (its `STATUS.md`, "A
  project's name"). Until both land, the default document seeds project
  1 as before, the playground's start and the first repository adopt
  into it, and the client shows it as "Project 1"; the mockups draw the
  target, marked "ahead · jig".

### 2.5 The box's terminal

With the user, 2026-10-23. Amends 3.9; the mechanics are
`../protocol/setup.md`, 7.2.

- **Why it changed.** A run on a terminal printed serve's notes and the
  sign-in link mixed, and the link itself cut by them: serve writes it
  in pieces, and the box passed each piece of serve's stdout on as it
  came, while it wrote the steps' stderr a line at a time. Pasting it
  meant cleaning it by hand. And most of what the steps say (smith's
  trust roots, the forges not connected yet) is nothing the person acts
  on.
- **Foreground, the only way.** `temper box` runs where it is started,
  and Ctrl-C stops it: no detached mode, no pid file, no `temper box
  down`. A process that shows what it started and where its logs are is
  one a person trusts, and it is what servers people already run do. A
  detached box would be a supervisor beside the system's: its pid file
  stale after a crash or a reboot, a status command to ask whether it
  runs, a restart at boot expected of it. The background way is the
  system's supervisor (`../protocol/setup.md`, 7.4; section 7), and a
  container runs the box in the foreground anyway.
- **Quiet.** The terminal shows the box's own lines: what it wrote and
  started, each group's start, end and failure, a step's restart, the
  stop. Each step's own lines go to its log file in the data directory,
  the last two runs kept; `--echo steps` shows them on the terminal
  too, as before. A failure shows its step's last lines and its log's
  path, so the person rarely opens a log.
- **The link is pasteable.** On a line of its own, whole, after what it
  is and how it behaves: it signs in once, for a short while, and a
  spent one opened again prints a fresh one, which the terminal
  announces. stdout stays the links alone, for scripts and tests; no
  log file holds one.
- **Times in UTC** on the lines after the link, since the box reads no
  time zone; local time is open (section 8).

## 3. The pieces

| Piece | Its home | Useful alone for |
|---|---|---|
| A command's arguments, environment and defaults (3.1) | skein: `shell.md`, 6.5 | every binary of every repository |
| Configuration, data and cache directories (3.2) | temper: `protocol/configuration.md`; jig for its parts' paths | every deployment, with or without the box |
| `temper init` (3.3) | temper: `protocol/setup.md`, section 8 | a person, a test or QA starting a deployment |
| The fakes as product crates (3.5) | skein: `crates/`, from `testing/` | QA, the real loop, demos, a person trying a charter |
| The fakes as the box's steps (3.5) | temper: `protocol/setup.md`, section 7 | QA's "temper in a box" (`qa.md`, 4.1) |
| Adoption from a client tool (3.6) | temper: `../client/tools.md` | scripts and agents adopting repositories |
| `temper forge fetch` (3.7) | temper: `protocol/setup.md`, 6.1; skein: the pins | any owned Forgejo |
| A dormant owned forge (3.8) | temper: `protocol/setup.md`, sections 5 to 7 | any deployment offering an owned forge later |

### 3.1 Arguments, environment and defaults

skein's rule for every binary (skein's `shell.md`, 6.5, landed
2026-10-20): a flag over an environment variable, `TEMPER_<NAME>`, over
a default; directories defaulting to XDG's base directories. temper's
commands adopt it with their next change.

### 3.2 The directories

| Directory | Flag, variable | Default | Holds | Backed up |
|---|---|---|---|---|
| configuration | `--config`, `TEMPER_CONFIG` | `$XDG_CONFIG_HOME/temper` | the document, `temper.json`; the charters' texts and the files the document names as configuration | as dotfiles are |
| data | `--data`, `TEMPER_DATA` | `$XDG_DATA_HOME/temper` | the store, jig's secrets' store, the worker's secret and digest, each set-up forge's directory, programs' login directories | yes: it is the deployment |
| cache | `--cache`, `TEMPER_CACHE` | `$XDG_CACHE_HOME/temper` | fetched binaries (3.7) | no: refetched |

- **Every command that reads the document** takes the three: `temper
  serve`, `temper forge`, `temper box`, `temper init`, `temper
  program-login`. A positional `CONFIG.json` goes.
- **Every path in the document says its directory** by its type: a
  configuration path, a data path or a cache path, each relative to its
  directory, or absolute. Today paths are relative to the working
  directory (configuration.md, section 2); the root's own paths and
  jig's parts' are both concerned, so jig's parts take the same three
  roots.
- **Secrets never sit in the configuration directory,** which people
  copy, sync and publish; the worker's secret moves to data.
- **The box passes its three on** to each step, as flags.

### 3.3 `temper init`

`temper init [--config DIR] [--data DIR] [--cache DIR]` writes a
deployment that starts:

- **The document,** from the default of 3.4, and the charters' texts it
  names; refused, naming the file, when the configuration directory
  holds a document already.
- **The worker's secret and its digest,** in the data directory, private.
- **The clients' listener's port, chosen once:** the default port when
  it can be bound, or one the kernel assigns, written into the
  document. The document always names a fixed port, as a configured
  GitHub forge requires (configuration.md, section 8); a later start on
  a port taken refuses, naming it and the document, as any start does.
  An owned forge's port is chosen the same way, and so are the fakes'
  and the webhooks'. Under `temper box` the sockets init chose are
  kept: the box holds every step's listener from its start and gives
  each step its own, so that no port is free between init and the
  step, and a second first run beside the first chooses other ports
  (setup.md, 7.1 and 8.2).
- **Nothing else:** no account, no forge credential, no repository,
  which are runtime's.

`temper box` with an empty configuration directory runs it first, and
says so; otherwise it never writes the document.

### 3.4 The default document

The first default is a working minimum, for the mechanics and the
experience; good defaults for a person's real work are open (section 8).

- **LLM providers:** OpenAI's, as the owned example's (setup.md, 8.1),
  with no account; and the demo model, an endpoint on loopback in
  plaintext, as the real loop's fake LLM peer is reached.
- **Forges,** each by its stable number: 1 the demo forge, 2 GitHub, 3
  an owned Forgejo, dormant (3.8). GitHub's addresses are its defaults;
  it starts not connected (`draft/github.md`, section 4).
- **No project** (2.4): one policy, which each project made starts
  from, and the deployment's landing rule, which lands on green CI with
  no human approval in every project; and one chat charter.
- **The box's steps:** the demo model and the demo forge, `temper
  serve`, and the owned forge's steps, dormant.

### 3.5 The fakes, as product

- **skein's fake LLM peer and fake forge move to `crates/`,** product
  crates on the same footing as the others, which the worlds and the
  real loop use as now. They keep their contracts (skein's
  `fake-llm.md`, `fake-forge.md`); only their place, and the guarantee
  that they ship, change.
- **temper serves each as a command,** `temper fake llm` and `temper
  fake forge`, a long step of the box, its address and its script or
  seed from the document. The setup's `mode` gains `fake` beside
  `managed` and `owned`: a forge the box runs from temper's own binary,
  whose preparation writes a handover before it starts. The engine's
  setup host takes the handover, reconciles into records, runs
  start-time adoption, then connects the fake from those records
  (`../protocol/setup.md`, 6.5 and 8.4). Adoption grants the bot write
  access and hooks each repository to the configured webhooks'
  listener.
- **The demo model's script** is the document's: suggested requests
  each with its scripted turns, and an answer for anything else, which
  says it is the demo model and offers to connect a real one.
- **The demo's state** resets at every start of the box (but see
  section 8).

### 3.6 The demo's seed

- **The repository** is the fake forge's seed: `playground/widget`, a few
  files and a check workflow the fake's CI answers.
- **Adoption is the demo's start's,** never configuration's:
  repositories are never configured (configuration.md, section 8). The
  engine adds the demo model's key and adopts `playground/widget` at
  the person's choice (2.3; setup.md, 8.4), where the box's seed once
  ran `temper ctl` to do it.

### 3.7 `temper forge fetch`

- **Fetches the pinned binaries** of Forgejo and its runner into the
  cache directory, when missing: `skein-forge`'s `VERSION` and
  `RUNNER_VERSION` (setup.md, 6.1), from their releases.
- **Checks each one's SHA-256,** pinned beside its version: setup.md
  15's digests move from later to now, required for fetched binaries.
- **A once step** of the box, before an owned forge's preparation, and a
  command of its own. An owned forge's binaries become a path, as now,
  or `"fetched"`, which names the cache's.

### 3.8 A dormant owned forge

- **An owned forge in the document may be dormant:** the box knows its
  steps and starts none until the person asks, and the client shows Set
  up a Forgejo here.
- **The person asks from the client;** the engine asks the box. The box
  is the engine's parent, so the ask goes over a pipe the box gives
  `temper serve` at spawn, carrying one request: start a dormant forge
  the document names. The box runs fetch, prepare and run, records the
  forge as started in its data directory, so that later starts run it,
  and answers. Run without the box (setup.md, 7.4), the client names
  the commands to run instead.
- **The engine takes the handover at runtime,** through the same
  connect a pasted managed admin's token takes (`draft/github.md`,
  12.2), where today it takes it only at its start.

### 3.9 The box, changed

- **`temper box [--config] [--data] [--cache] [--echo]`,** no positional
  argument; init when empty (3.3).
- **Its steps,** from the document: the fakes' (3.5), fetch, prepare
  and run for each started owned forge, then `temper serve`; a dormant
  forge's when asked (3.8). Its policy is section 7.2's, unchanged.
- **Its stdout** is `temper serve`'s links alone, each whole; its
  terminal is quiet, and each step's lines are in its log (2.5).

## 4. What needs a restart

| Change | Restart? |
|---|---|
| An LLM account added, any provider in the document | no: jig's accounts at runtime |
| GitHub connected, managed or connected | no: in the document, connected at runtime (`draft/github.md`, section 4) |
| An owned Forgejo set up | no: dormant in the document (3.8) |
| A Forgejo elsewhere, managed or connected | yes: its address is the document's. The client says what to add and that temper restarts |
| Anything else in the document | yes, until a hot restart (section 7) |

## 5. Order

1. **The arguments' rule:** its function in skein's shell (its design
   landed, `shell.md`, 6.5); each binary adopts it with its own next
   change.
2. **Directories, `temper init`, the box changed,** on the default
   document without the fakes: the person signs into an empty project.
   Waits for the accounts' second part and the GitHub work to land.
3. **The demo, mechanics first:** the fakes moved and served, the
   `fake` mode, the seed and the box's client tool. Its bar is a chat
   that works on the demo model, and the demo forge registered and
   working as far as today's pieces allow: the fake git program on it
   (8), its state gone at a restart as it happens to be. Section 8's demo questions,
   the defaults and the content are its follow-up.
4. **Going real from the client:** fetch, the dormant owned forge, the
   box's pipe, the handover at runtime, and the client's banner and
   wording; and the first run's ports, each step's listener held by the
   box from its start and given to the step (3.3; setup.md, 7.1), which
   rests on skein holding, giving and adopting a listener (skein's
   `io.md`, 3.2 and 6.3; `shell.md`, 6.6), Forgejo's on Linux, where
   forge run stays its parent (setup.md, 6.4). Built; the process tier
   walks it whole on fresh scratch, the playground then a Forgejo set up
   from the client, with no restart (setup.md, 13.4, "The first run
   whole").
5. **The welcome and the demo's end** (2.3): adopted, closed here.
   The demo's state, the fakes asked, the seed's acts and the end in
   the engine, `stop` on the pipe (setup.md, 7.1, 7.2 and 8.4;
   `../domain/forge.md`, 4.1), the welcome, its wizard and the halves
   in the client (`../client/README.md`, 3.5 and 4.8), and the
   playground's words in the fake's scripts, `temper init`'s document
   and the client; what is built is `STATUS.md`'s.

## 6. Testing

- **The process tier** (setup.md, 13.4) runs `temper box` on empty
  scratch directories with every directory flag given, and checks the
  link signs in and the demo project's chat answers a suggested
  request. No test uses the defaults under `$HOME`.
- **`temper init`** in step tests: the document it writes checks as
  configuration.md, section 8 says, port chosen and port fallen back.
- **The arguments' rule** in skein's own tests: flag, variable, default,
  an empty variable.
- **Fetch** against a local file server with a wrong digest, refused.

## 7. Later

- **The demo forge's pages:** its pull request's link now answers 404.
  Either the demo forge serves a few read-only pages, or temper shows a
  change's diff itself, which would also show more of a change's
  progress on any forge (`../client/forge.md`).
- **A hot restart:** the process stays, its listeners and connections
  with it, and the domain is built again from a new document as a start
  builds it. It would make section 4's last rows no restart. An
  investigation of the code (2026-10-20,
  `hot-restart-report.md`) finds it
  feasible, medium for a first version, large for any change at any
  time:
  - **The seam exists:** `Service` keeps io, the store and the worker
    apart from `Engine`'s domain, in temper as in jig's reference
    service; skein's loop needs nothing.
  - **What is new is a lifecycle,** pause, drain, rebuild, recover,
    rebind, distinct from shutdown, which stops clients and workers,
    and from a cold `Restart`, which jig's core takes only once.
  - **"No task in flight" is not needed,** since recovery already
    resumes durable work; but jig's adoption treats an in-process
    worker's attempts as lost at a start, so a worker that lives on
    needs a rule of its own, and a first version would require no agent
    attempt running.
  - **Watches are not stored,** so the clients' watches are rebound
    from their full subjects, which jig's protocol does not carry yet.
  - **The first version:** policy, charters, templates and limits
    within the same capacity; the same deployment, listeners, workers
    and endpoints. jig owns the lifecycle and its tests; temper its
    configuration's diff and its wiring.
- **The box in the background:** should people want their terminal
  back, a command that writes a systemd user unit for the box
  (`../protocol/setup.md`, 7.4), never a pid file of temper's own
  (2.5).

## 8. Open questions

- **The demo's reset against a lasting store.** The fake forge's state
  is in memory, but the engine's store lasts: after a restart it holds
  an adoption, tasks and pull requests the fresh fake forge lacks.
  Either the fake forge keeps its state in its data directory, or a
  start forgets the demo forge and its project's work and seeds them
  again. Default: as it falls in the first iteration (5, item 3);
  answered in its follow-up.
- **git on the demo forge.** The fake forge's git is reached through a
  typed transport, never over HTTP (`fake-forge.md`, section 3), and
  the real loop runs a fake git program (`../domain/testing.md`, 4.3).
  The demo needs a git that reaches the fake forge across processes,
  not real git: its agent is the fake LLM, and nothing else reads its
  working trees. Default: the fake git program as a program of its
  own, with its transport over the network, as the QA loop needs it
  (`qa-loop.md`, section 11, item 2); the demo then needs no git on
  the machine. Real git against the fake forge is real-loop's later
  real-git bridge, for real agents on the fake forge (`../STATUS.md`).
- **The default port.** Default: one unassigned port, chosen with the
  default document.
- **Good defaults** for a person's real work: charters, landing rules,
  limits, the memory bound (setup.md, section 16). Default: the first
  default of 3.4, refined once the mechanics run.
- **The demo's content:** the repository, its CI, the suggested
  requests and their scripts. Default: one request, refined later.
- **The binary's commands:** engine alone, worker alone, standalone,
  the box; their names and how the box names what it starts. Default:
  `temper box` and today's commands, until a session of its own.
- **A Forgejo elsewhere without a restart:** the client writing the
  document and the box restarting `temper serve`, or the hot restart.
  Default: the person adds it and restarts.
- **The terminal's times in local time** (2.5): the machine's time zone
  read by the box. Default: UTC, marked `Z`.
