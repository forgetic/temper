# Step 05s: smith

Provisional, 2026-10-05. temper's agent leaves for smith: the kit for
flexible LLM agents, and the standard agent built from it, in its own
repository (`~/src/rust/smith`, on the forge as `ai/smith`), designed in
smith's `docs/design/domain/` (its first commit). temper's `agent.md`
says what temper takes from smith and fills in. This step builds smith
beside temper's agent, freezes temper's agent as legacy for the overlap,
and replaces 05f and 05g (05-runtime.md, section 4): the run's and the
tools' new parts and the agent's protocol translation are built in
smith, generic, rather than in temper. Overview and conventions:
README.md.

## 1. Why beside, again

- **The overlap needs temper's agent as it is.** The system worlds run
  the legacy engine, the worker and temper's agent on the channel's first
  version until the cutover (README.md, 2.2). smith speaks only its own
  channel and the conversation vocabulary's second version. Converging
  temper's agent into smith in place would make one crate serve both, the
  hybrid the plan avoids everywhere else.
- **So smith starts from a copy,** as the new root is built beside the
  old: the agent's crates as they are after 05e, its providers, its OAuth
  client, its fake LLM and their worlds; and 05d's agent child of the
  worker, for smith's host domain. What the parked 05f branch built of
  the run and the tools is ported where it fits smith's design, not
  merged.
- **temper's agent freezes,** renamed as legacy (README.md, 3.4), until
  the cutover switches the system worlds to smith's agent (07b) and
  deletes it (07d).
- **The root does not wait.** Its runs, charters, tools and messages are
  its own types (06-root.md, section 6); smith appears only at the
  engine's protocol layer (07a) and in the system worlds (07b).

## 2. smith's repository

- **Layout,** as skein's and temper's: `crates/` for domain and protocol
  crates, `testing/` for fakes, `tests/` for worlds and nothing under
  `crates/*/tests`, `docs/design/domain/` (there already),
  `docs/development/workflow.md`, `AGENTS.md`, `README.md`.
- **Conventions,** as temper's: the profiles and lints of
  programming-model.md, section 10; `rustfmt.toml` and `clippy.toml`;
  the four checks of its own `workflow.md` before main moves, a change
  to Markdown only skipping them; two nextest suites in
  `.config/nextest.toml`, the default within **15 seconds** and the fuzzy
  within **60**; increments as branches merged `--ff-only`; review as
  README.md, section 8 says; code cites its design as
  `domain/<file>.md`, section by section.
- **skein by git,** as temper takes it. temper takes smith the same way,
  `smith-*` from `https://git.ekanayaka.io/ai/smith.git`, its revision
  pinned in `Cargo.lock`; both resolve one skein revision.
- **Changes across both** land in smith first, through smith's gate, and
  temper takes them by bumping its lock in an increment of its own,
  through temper's.
- **The forge:** `ai/smith`, with a GitHub backup, as temper's remotes.
  The forge's admin makes it; until then smith's branches stay local.

## 3. Names

| temper today | smith's copy (05s2, 05s5, 05s6) | temper's, for the overlap (05s3) |
|---|---|---|
| `temper-agent-domain` | `smith-domain` | `temper-legacy-agent-domain` |
| `temper-agent-domain-run` | `smith-domain-run` | `temper-legacy-agent-domain-run` |
| `temper-agent-domain-session` | `smith-domain-session` | `temper-legacy-agent-domain-session` |
| `temper-agent-domain-tools` | `smith-domain-tools` | `temper-legacy-agent-domain-tools` |
| `temper-agent-protocol` | `smith-protocol`, its agent half | `temper-legacy-agent-protocol` |
| `temper-llm-anthropic`, `temper-llm-openai` | `smith-llm-anthropic`, `smith-llm-openai` | `temper-legacy-llm-anthropic`, `temper-legacy-llm-openai` |
| `temper-oauth` | `smith-oauth` | kept by its name: the engine's protocol layer uses it until 07c |
| `testing/temper-fake-llm-domain`, `-protocol` | `smith-fake-llm-domain`, `-protocol` | `testing/temper-legacy-fake-llm-domain`, `-protocol` |
| `temper-worker-domain-agent` | `smith-host-domain` | kept, extended, until step 08 swaps the worker's child |
| `tests/agent/{tools,session,run,protocol}`, `tests/fake-llm` | smith's `tests/{tools,session,run,protocol,fake-llm}` | `tests/legacy/agent/{tools,session,run,protocol}`, `tests/legacy/fake-llm` |
| `tests/agent/domain` | smith's `tests/agent`, on a scripted host | kept by its path: a system world, moved at 07b |
| `tests/worker/agent` | smith's `tests/host` | kept |

## 4. Increments

### 05s1 smith's workspace (smith)

The workspace and its conventions (section 2), with no agent yet: the
world harness smith's worlds need (`temper-world`'s settings, replay and
expectations), taken from skein if section 6 has put it there, or copied
and owed to skein. The gate passes on it.

### 05s2 the agent copied (smith)

- **From a named temper commit,** which the commit message gives: the
  four agent crates, the providers, the OAuth client, the fake LLM and
  their worlds, under smith's names (section 3), with `use` lines and
  citations repointed to smith's documents.
- **The top-level world rehosted:** temper's ran the agent under the real
  worker and engine; smith's runs it under a scripted host (smith's
  `host.md`, section 2).
- **Behaviour unchanged:** every copied story passes as it did, measured
  with smith's serial profile, and the shares recorded in smith's
  `workflow.md`. The protocol crate waits for smith's channel (05s5).

### 05s3 temper's agent moved aside (temper)

As 00c moved the engine: the crates and worlds of section 3's last
column renamed, mechanical, no behaviour changed; `tests/agent/domain`
keeps its path and takes the new names. From here they change only as
README.md, 3.4 allows.

### 05s4 the run and the tools, generic (smith; replaces 05f)

smith's `run.md`, as its section 14 lists, each part its own increment:

- **results:** the contract's four forms, with fields and items; a
  change's title and body as fields; temper's verdict lists gone from code;
- **delivery:** push generalised to `deliver`, its outcomes delivered,
  nothing, refused, failed and stale, its 512-byte diagnostic kept;
  delivering mid-run; merges in progress, a marker refused by name;
- **host tools:** the closed `Ask` opened, calls relayed whole, named,
  asked again after busy or a lost answer; the forge and outlet grants
  gone;
- **messages, `wait`, parking, resuming,** on 05e's transcripts;
- **conventions, an optional workspace, the brief** as titled sections;
- **the budget** in a unit with prices, the token split gone;
- **the session's first version** contracted, once the run opens only
  the second.

Stories: smith's `run.md`, section 13, in smith's run and agent worlds.

### 05s5 the channel, the protocol and the agent (smith; replaces 05g's agent half)

- `smith-channel`: the contract's frames and payloads (smith's `host.md`,
  sections 2 and 3), from the agent hop of temper-channel's second
  version (05b; `channel.md`, section 16), without temper's names; golden
  frames with a documented regeneration command and drift tests.
- `smith-transcript`: turns' encoding, from 05e's versioned records.
- `smith-protocol`: the agent's half of `temper-agent-protocol`:
  providers, tools' schemas, decoding and rendering, `finish`'s schema
  generated from the contract, host tools' schemas passed through;
  without temper's half (the brief's rendering, the engine's charter's
  mapping), which 07a writes in temper.
- `smith`, the binary, as an agent process on its pipes.

### 05s6 the host domain (smith)

`smith-host-domain` from `temper-worker-domain-agent` as 05d extended it,
with temper's names gone, and its world from `tests/worker/agent`, on a
scripted agent.

### 05s7 temper's half of the runtime (temper; replaces 05g's worker half)

- **`temper-channel`'s second version** carries smith's charter as bytes
  in an assignment, and drops the agent hop's payloads, which are
  smith's; the second version has not shipped, so it changes freely
  (`channel.md`, 4.5).
- **The worker** passes the charter through and starts the agent with
  smith's start (worker.md, 4.1); its protocol translation of the second
  version's engine link, as 05g was to write it.
- **The worker's agent link** below the domain is smith's channel's host
  half, built with the rest of the worker's protocol and io (08-after.md,
  section 3).

### 05s8 the local host domain (smith)

`smith-local-domain` and its world, new rather than copied (smith's
`host.md`, sections 8–11). This completes the host domain logic the new
goal includes alongside the extracted agent:

- typed configuration, terminal messages and cancellation, optional
  workspace, transcript persistence and delivery in place;
- the world with smith's real agent, fake LLM, scripted person and fake
  disk: a workspace-free chat waits, parks and resumes across invocations;
  a change is checked and committed in place; terminal cancellation settles;
- in-process composition through small total translations, preserving the
  same call, turn, acknowledgement, budget and cancellation contracts
  (smith's `host.md`, section 9).

Files, terminal IO and contained process trees remain typed domain
boundaries, exercised through fakes. A live terminal adapter is a separate
lower-layer implementation; this increment completes the local host's
domain logic and worlds. MCP remains explicitly later in smith's design.

## 5. Tests and budgets

- **smith's worlds** start as copies of temper's (README.md, 5.5: the
  agent's run, session, tools and protocol worlds, the fake LLM's tests,
  the worker's agent world) and grow by smith's documents' stories,
  within smith's own budgets.
- **temper's legacy agent worlds** keep running, frozen, until 07d, their
  fuzzy seeds trimmed if a budget needs room (README.md, 3.4); they leave
  with the legacy engine, and README.md, 5.5 records the new shares.
- **The system worlds** pass unchanged, on the first version, through
  this step.

## 6. What goes to skein

What smith and temper would otherwise each keep goes to skein, when the
second copy would appear, as an increment in skein first and then one in
each of the others to take it:

- **the world harness** (`temper-world`'s settings, replay, expectations,
  referee plumbing): before or with 05s1;
- **framed channels** (a link's frames, its hello with versions, bounds
  sealed by constructors, the channel's state machine): once 05s5 has
  smith's beside temper's;
- **an OAuth client** (sign-in, refresh, tokens as secrets): from
  `smith-oauth`, when temper's web signs people in with the forge
  (08-after.md, section 3);
- **supervised process trees** (spawning within a deadline, cancel then
  terminate then kill, proof that a tree is empty): from
  `smith-host-domain`, if temper's worker needs them beyond its agent
  child.

Each follows skein's own conventions and review; what is about LLMs,
conversations, tools or runs stays in smith, and what is about tasks,
authority or connectors in temper.

## 7. Order

```
05s1 ──► 05s2 ──┬──► 05s4 (several) ──┐
                ├──► 05s6 ────────────┼──► 05s5 ──► 05s7
                └──► 05s3 (temper)    │

05s4 + 05s6 ──► 05s8 (local host)
```

The step starts after README.md, section 8's gates (the walking story,
the tasks audit, the documentation backfill and the style checks). 05f
and 05g stay parked meanwhile and are ported, not merged. 07 needs 05s
done, including 05s8. The local host follows 05s4 and 05s6 and may be
built beside 05s5–7.

## 8. Done when

- smith's gate passes, with every story of its documents' worlds,
  including the local host's (05s8); its agent, run, session, tools, host
  and local host domains conform to their designs;
- temper's agent is legacy and frozen, and the system worlds pass
  unchanged on the first version;
- smith's documents and code agree, and temper's `agent.md`, section 11
  says what is done.
