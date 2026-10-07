# Continuous QA

Draft, 2026-10-07. Agents and procedures that keep using a build of
main, on every landing or on a period, to check it still works and to
find bugs, with temper as its first subject. This document says what
continuous QA is for, given temper's worlds; which of temper's and jig's
parts it is built from; what is new; where it lives; and in what order
it could be built. Nothing here is adopted: `docs/design/domain/` and
jig's documents describe what is built. What is still open is listed in
section 10.

## 1. In one page

- **Aimed past the worlds.** temper's domain is complete in its worlds,
  with fakes, crashes at every commit and a referee
  (programming-model.md, section 4). QA on a running build would not
  find domain bugs faster than the fuzzy suite does. It is for what the
  worlds cannot reach: the fakes' fidelity, the layers below the domain,
  the agents' quality, the experience of using it, and long horizons
  (section 2).
- **Every finding ends in the worlds.** A bug found on a running build
  is a story the worlds lack, a fake that differs from its system, a
  bug below the domain, a charter's regression, or a problem of use.
  Its fix carries a world scenario, a pinned seed or a fake's
  correction, so that QA feeds the worlds rather than competing with
  them.
- **Built from parts that exist.** Landings as news, standing and
  recurring procedures, test environments as a connector, pooled holds,
  findings as issues, requirements judged by another connector, priced
  effects and services as parties are all in temper's or jig's design
  already (section 3).
- **What is new** is small:
  - an environments connector, whose first kind is temper in a box;
  - a driver, through which an agent uses the product as a person
    does;
  - oracles beyond an agent's opinion;
  - findings as state of their own;
  - a few charters and procedures (section 4).
- **In temper, as a deployment of its own.** QA is part of a software
  factory, and its loop closes there: finding, goal, change, landing,
  checked again. Independence comes from a separate deployment pinned
  to a trusted release, not from a separate codebase (section 6).
- **A project of its own** only if QA comes to serve products temper
  does not build, or people who want no factory (6.2).
- **Later than the domain.** temper runs only in its worlds until the
  layers below its domain exist (`docs/plans/next-domain/08-after.md`,
  section 3), so QA is built in steps, the first needing no agent
  (section 8).

## 2. What it is for

What the worlds cannot reach, and so what QA aims at:

1. **The fakes' fidelity.** Where the fake forge differs from a Forgejo
   release, or a fake LLM from a provider's API. `tools/forgejo-conformance`
   checks this by hand today, against one version.
2. **Below the domain.** The protocol and io layers: real processes, a
   real `kill -9`, real disks and the store's files, webhooks that
   arrive late or twice, a network that drops.
3. **The agents' quality.** Whether the coder's and the reviewer's
   charters still land fixes after a change of charter or of model.
   These are evals: statistical, priced, never pass or fail on one run.
4. **Use.** Whether a person gets from "I want X" to a goal, a
   proposal accepted and a landing, without getting lost in the web.
5. **Long horizons:**
   - soak: a deployment running for days, its memory and its store's
     growth watched;
   - upgrades: state made on release N, the deployment upgraded to
     main, every record's version still loading and every live task
     going on.

What it is not for: the stories of the domain, which the worlds own. A
QA story that keeps finding domain bugs says that a world lacks
stories.

## 3. Built from what exists

| QA needs | Already in the design |
|---|---|
| a run on every landing on main | the forge's topic of landings on a branch (`domain/forge.md`, 7.1), and a standing procedure subscribed to it (jig's `connectors.md`, 6.6) |
| a run every night, within a budget | the recurring procedure, its budget carved each period (`domain/tasks.md`, section 9) |
| an environment made from a head | test environments as a connector (`domain/connectors.md`, section 14); `ops`'s provision and tear-down (jig's `examples.md`, 3.2) |
| few environments, shared | holds pooled as counted slots, a taken one waited for (jig's `tasks.md`, section 6) |
| agents using a running build | T10, an agent with a test environment (`domain/core.md`, section 9) |
| aiming where a landing changed | overlap by files, as the forge classifies landings (`domain/forge.md`, 7.3) |
| bugs filed | findings as issues, keyed; taking one up is a goal (`domain/forge.md`, section 12) |
| QA holding a landing | a requirement judged by the connector whose facts it is (jig's `authority.md`, section 10; jig's `connectors.md`, section 7) |
| environments' time paid for | priced effects (jig's `authority.md`, section 7) |
| QA as an identity | a service, a party of its own kind (jig's `people.md`, section 3) |
| a second deployment on temper's forge | a branch prefix per deployment (`domain/connectors.md`, section 11) |

## 4. What is new

### 4.1 Environments

`domain/connectors.md`, section 14, made a design: temper's second
connector, close kin of `ops`'s infrastructure connector.

- **Resources:** pools, environments in them, and deployments in an
  environment, each of a head of a repository.
- **Temper in a box,** its first kind: a throwaway Forgejo, made as
  `tools/forgejo-conformance` makes its scratch instance; an engine and
  a worker built at the head; an LLM endpoint; seed state (an owner,
  repositories to adopt, accounts).
- **Two levels of LLM:**
  - *scripted:* a fake endpoint over the providers' real protocol,
    cheap and deterministic, for the plumbing (section 2, items 1, 2
    and 5);
  - *real:* models at their prices, for evals and use (items 3 and 4),
    on credentials of their own, capped.
- **Effects:** deploy a head, a keyed creation; tear down, as the
  release of the task that held the environment; inject a fault (kill a
  process, stall the network, fill a disk) for chaos.
- **Procedures:** provision, then wait until ready; deploy, then check,
  whose result is a verdict at the head it deployed.
- **Reads for agents:** the box's logs, its processes and their memory,
  the throwaway forge's objects.
- **Workspaces:** none of its own. A build is a run in a checkout, as
  any; what it produces is named for the deployment to take.

### 4.2 A driver

How an agent uses the product as a person does. For temper, and for any
application on jig, the design has it already: jig's client domain, on
the native shell over skein's HTTP client, read as a text view by role,
name and text, as the worlds' fake person reads it (jig's `README.md`,
section 10).

- **As a host tool,** an explorer acts through the same interface the
  fake person scripts act through, so a path an explorer found is
  written down as a fake person's script in a world, the finding's
  regression test.
- **Not the browser.** The wasm shell and the page's rendering need a
  driver of their own, later.
- **Other products** need other drivers: a browser, HTTP, a command
  line (section 10).

### 4.3 Oracles

What says a bug was found, beyond an agent thinking so:

- **A stop or a panic** in the box's logs. A failed commit stops the
  engine by design (`domain/core.md`, section 4), so every stop is
  read.
- **temper's promises, seen from outside** (`domain/core.md`,
  section 10), on the throwaway forge, as the referee sees them in the
  worlds: two creations under one key; a forced push; a write to an
  object temper does not own; a merge at a head its decision did not
  see.
- **Memory against the declared worst cases** (`domain/engine.md`,
  section 13): an engine or a worker past its own declared bound is a
  finding, which soak gets for free.
- **Result contracts** of scripted paths.
- **Baselines:** spend per change landed, turns per task, the time from
  a request to a landing, compared with their history.
- **An agent's judgement,** for use: a task it could not see how to do,
  a page that misled it.

### 4.4 Findings

More than an issue: state of their own, kept in the store, which an
issue projects (`domain/forge.md`, section 12).

- **A signature,** for deduplication: a panic's message and place, a
  promise and its subject, or a triager's judgement against the open
  findings.
- **Flaky or not:** reproduced again before it is filed, a number of
  times, and classed.
- **Bisected:** a procedure over the landings between the last head
  that passed and the first that failed; each step deploys a head and
  reproduces. It is mechanics, with no LLM.
- **Classed** by where its fix goes (section 1): a world's story, a
  fake, below the domain, a charter, use.
- **Checked again** at the landing that fixes it, and closed; reopened
  if it comes back.

### 4.5 Charters and procedures

- **Charters:**
  - *explorer,* per area or per persona (a new owner, a maintainer, a
    contributor outside the project), aimed by what a landing changed;
  - *chaos,* which injects faults and drift (a hand's push, a pull
    request closed) and reads the oracles;
  - *triager,* which deduplicates and classes findings;
  - *eval runner,* which runs a charter under test many times and
    reports its rates.
- **Procedures:**
  - the *watch,* standing, subscribed to the landings on main, which
    makes a smoke and an exploration per landing, coalescing a burst to
    its last head;
  - *deploy, then check* (4.1);
  - *bisect,* and *check again* (4.4);
  - the *recurring* nights: evals, soak, an upgrade from the last
    release.

## 5. A finding, as tasks

```
Q, the QA deployment's service
├─ W   watch temper's main                    procedure: standing, subscribed to landings on main
│  ├─ S   smoke at h7                         environments' deploy, then check: scripted paths
│  └─ X   explore landing queues at h7        agent, an explorer; an environment from the pool held
│     └─ F   finding: two merges, one change  triager: a new signature, reproduced three times of three
│        └─ B   bisect h3..h7                 procedure: deploy and reproduce at each landing
└─ N   nightly                                recurring: evals, soak, an upgrade from the last release
```

- **h7 lands.** W hears it and makes S and X, both at h7; X's charter
  is aimed at the landing queue, which h7 touched.
- **X finds** a change merged twice after a restart, by the promise of
  once, seen on the throwaway forge. F is made, reproduced, and B finds
  h5 as the first head that fails.
- **F is filed** as an issue on temper's home repository, naming h5,
  the repro and its class: below the domain, the store's protocol.
- **Someone takes it up:** a person, or temper's development deployment,
  for which the issue is a participating object (`domain/forge.md`,
  section 13). Its change carries a test at the store's protocol layer.
- **The fix lands at h9.** W hears it; F is checked again at h9, passes,
  and closes, its issue with it.

## 6. Where it lives

### 6.1 In temper, as a deployment of its own

- **In temper's code.** The environments connector, the driver as a
  host tool, findings and the charters are temper's: QA belongs to a
  software factory, and the environments connector is the second
  connector temper's design expected (`domain/core.md`, section 1).
- **A deployment of its own,** pinned to a release that is trusted,
  testing builds of main inside its environments, so the tester never
  runs the code it tests:
  - its own store, budget and branch prefix;
  - a service as its identity;
  - temper's repository adopted as context, its home repository for
    findings' issues;
  - authority to read, to file issues, and to use its pools; no push and
    no landing on temper's repositories.
- **Findings cross by the forge.** The QA deployment files issues; the
  development deployment, or a person, takes them up. Neither
  deployment needs the other's store.

### 6.2 A project of its own

A jig application of its own, `assay`, would be warranted:

- **when QA serves what temper does not build,** or people who want no
  factory. This is the trigger;
- **not by sophistication alone.** Campaigns, coverage maps, evals'
  baselines and "is head X releasable" are objects that are not tasks,
  but jig lets an application keep them as child domains and client
  objects of its own (jig's `README.md`, section 4), inside temper.

If it were split:

- **Connectors would overlap** temper's forge and `ops`'s
  infrastructure, which asks jig whether two applications may compose
  one connector's crate. Nothing forbids it; jig's documents treat
  connectors as each application's own.
- **It would not replace `ops`** as jig's second application: it shares
  too much with temper (a forge, workers, workspaces) to keep jig's
  vocabulary from being temper's in disguise.

## 7. Independence and authority

- **The tester is not the tested.** The QA deployment runs a release; what
  it tests runs in its environments, with their own forge, credentials
  and LLM endpoint.
- **No hand on the product.** QA's tasks may read temper's repository
  and file issues; they never push, land or edit a test, so a QA agent
  cannot make a failing check pass by changing what it checks.
- **Spend is a period's.** Continuous means within a budget per period:
  exploration goes on until the period's budget is spent, aimed by risk
  (what landed, what failed lately), and a priced environment's time
  counts with its LLM turns.
- **Real credentials, capped.** Evals' LLM accounts are QA's own, with
  limits of their own; the box never holds the development deployment's
  credentials.

## 8. An order of building

1. **Now, with no agent:** `tools/forgejo-conformance` on a schedule,
   against each new Forgejo release, its observations compared with the
   fake forge's. It finds fake gaps (section 2, item 1) before a
   protocol depends on them.
2. **Once the store, the web and the workers' protocols run:** temper
   in a box with a scripted LLM, and a few scripted paths (adopt a
   repository, ask a chat for a small fix, see it land). They may start
   outside temper, as a scheduled job, written so that their scripts
   and charters carry over.
3. **Once a release is trusted:** the QA deployment (section 6.1), its
   watch and its findings; then explorers, chaos and evals.
4. **Later, if wanted:** cheap scenarios as gates on landings, judged
   by the environments connector at the change's head (section 3).

## 9. What it asks of temper and jig

- **temper:** the environments connector, designed from
  `domain/connectors.md`, section 14; findings, as a child domain of
  temper's root or grown from the forge's issues; the charters.
- **jig:** a client shell an agent can use as a tool, which the
  client's design (jig's `README.md`, section 10) is to allow; whether a
  connector's crate may be shared by applications (6.2).
- **An LLM endpoint that is scripted** over the providers' real
  protocols, for the box, from whichever of skein, smith or temper keeps
  the fakes of LLMs.

## 10. Open questions

- **The subject:** temper and applications on jig only, or anything
  temper builds? It decides the drivers (4.2) and most of 6.2.
- **First goal:** the plumbing with a scripted LLM only, or evals of
  real models from the start.
- **After or before landing:** QA on main after landing, with bisect,
  only; or cheap scenarios as gates too (section 8, step 4).
- **Who acts on findings:** a person, or temper's development deployment
  making goals of them on its own, within a budget.
- **Verdicts over many runs:** how an eval's rate becomes a finding,
  against what baseline, and how many runs it costs.
- **Environments' lives:** made per run, per landing or kept warm in a
  pool; where they run (local containers, as `ops`'s real backend would
  have, or machines of their own).
