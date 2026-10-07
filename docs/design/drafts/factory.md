# The software factory

Draft, 2026-10-07. A vision of the whole factory: code, then proving
it, then deploying it, then operating it, on jig. It records the
direction of a design discussion on 2026-10-07, and it crosses
boundaries on purpose: temper, jig, os (`~/src/rust/os`) and
applications not yet started. Its concepts are meant to be generic and
pluggable. os is their first backend, not their definition. Nothing here
is adopted: `docs/design/domain/` and jig's documents describe what is
designed. Section 13 says what this changes in `qa.md`; section 14 lists
what is open.

## 1. In one page

- **The subject is services.** A mail server running postfix, a
  database cluster, temper itself. Most are not developed with temper,
  but their configuration is: it lives as IaC in a repository, agents
  change it through temper and the forge, and landing a change deploys
  it. This is GitOps (section 2).
- **Three planes, split by trust** (section 3):
  - **write,** which is temper: changes to code and IaC, landed through
    gates;
  - **prove,** the lab: runs any head on throwaway machines, with
    experiments, chaos, benchmarks and bisects;
  - **run:** brings production to what landed, watches it, rolls back.

  The lab runs code nobody reviewed yet, and the run side holds
  production's secrets. They never share a deployment.
- **The planes meet through the forge,** and through one application
  calling another (section 4). The lab is consumed passively, like CI,
  by statuses at heads and issues; or actively, through a connector in
  temper to the lab.
- **One ground layer under the lab and the run side** (section 5):
  revisions of IaC, targets, machines in pools, A/B slots. The lab and
  the run side use the same connector with different pools and policies.
- **Pay only for what changed** (section 6). Everything is keyed by the
  digests of its inputs, part by part. Patching is the default; a build
  from scratch is the fallback and the proof. A one-line change moves
  the bytes of its commit, not images.
- **One build daemon** (section 7): outputs from inputs, keyed by
  digest, with two recipes: a root filesystem from a target, an
  artifact from sources. It is the systems side of the factory, behind
  a connector, and knows nothing of jig.
- **The lab is the worlds, applied to real systems** (section 8):
  seeded faults, checkers, evidence accumulated per key, findings that
  end as pinned seeds in the product's repository.
- **Product knowledge is data in the product's repository:** how to
  build it, run it, use it, prove it and judge it healthy. The factory
  is generic; agents write and evolve that data through changes, like
  any code.

## 2. The subject: services, by GitOps

- **A service's repository is its IaC.** For postfix there is nothing to
  build: the service is Debian's packages plus configuration. What
  changes is a commit of the IaC repository: an os configuration
  pinning a Debian snapshot, postfix's settings, its hosts.
- **Every change to a service is a change to that repository:**
  - an agent editing configuration: "enable DKIM";
  - a recurring bump of the Debian snapshot, which is how security
    updates arrive;
  - the fix of an incident.
- **Software developed in the factory** adds a step upstream: its build
  is pinned into an IaC repository, like a package. A distributed
  database developed with temper, called `stoa` here, is the example.
- **Deploying is landing.** The desired state is the IaC repository's
  main branch. A change to it goes through the forge's change procedure,
  landing queue and gates (`domain/forge.md`, sections 8 and 9). Machines
  then converge to what landed (section 9). Promotion from staging to
  production is another change, gated on verdicts from staging.
- **Immutable systems.** A machine runs a root filesystem built from a
  revision, never a system modified in place. Rolling back is switching
  to the previous build.

## 3. Three planes

```
            write (temper)             prove (the lab)                 run
trust       pushes; no secrets         runs any head; no secrets,      landed revisions only;
                                       no push                         production's secrets; no push
works on    text                       throwaway machines              long-lived machines
does        changes and landings       experiments: smoke, chaos,      rollouts, watching, triage,
                                       benchmarks, soak, bisect        rollbacks
gives       commits                    verdicts at heads, findings,    health at commits, incidents
                                       trends
agents      on workers: coder,         in the engine: explorer,        in the engine: triager,
            reviewer                   triager, analyst                incident lead
```

- **Proving, deploying and operating are one activity at different
  stakes:** run a revision somewhere and watch it. The lab does it with
  made-up traffic on throwaway machines; a canary does it in production
  with real traffic; operating does it for as long as the service
  lives. They share their connectors (section 5).
- **Trust splits them anyway.** The lab runs code at any head, possibly
  from an outside contributor, so it holds no secrets that matter. The
  run side holds production's credentials, runs only what landed, and
  must stay up when everything else is broken. It changes slowly, from a
  pinned release.
- **Only write works on text.** temper is unchanged by this document:
  its forge connector, its workers and its charters serve IaC as they
  serve code.

## 4. Between the planes

```
temper ──heads───────────────────▶ lab ──verdicts at heads──────▶ temper's gates
temper ──landed IaC──────────────▶ run ──health at commits──────▶ temper's goals
lab ─────findings, regressions as issues────────────────────────▶ temper
run ─────incidents as issues────────────────────────────────────▶ temper
temper ──"run this experiment at this head" (active)───────────▶ lab
```

- **Through the forge, passively.** The lab watches the repositories it
  is given, runs experiments by its own rules (section 8), posts
  statuses at heads and files findings as issues. A status at a head is
  already a fact temper's gates read, as CI's is (`domain/forge.md`,
  8.3), so temper's forge connector does not grow. Anyone on the forge
  can consume it: a person, or another factory.
- **Through a connector, actively.** When a coordinator wants "benchmark
  this branch against main" as a step of its plan, not as a gate, temper
  has a connector to the lab: resources are experiments; an effect is a
  request, keyed and priced, for an experiment at a head with a level of
  evidence; its result is a verdict with its evidence. The lab's price
  is spent from the requesting task's budget (jig's `authority.md`,
  section 7). temper is then a client of the lab that is not a person
  (jig's `README.md`, section 10).
- **Done means proven in production.** A goal that changes a service can
  wait on the run side's health status at the commit it landed, and end
  only then.
- **Identities.** The lab and the run side are parties on the forge of
  their own, services (jig's `people.md`, section 3).

## 5. The ground: targets and pools

### 5.1 Names

The concepts are generic; the right column is os, their first backend.

| Concept | What it is | In os |
|---|---|---|
| IaC repository | a system's declarative inputs | `hosts/` (a base per host, a variant per environment) and `tasks/` |
| revision | a commit of it | |
| target | one host in one environment | a target, such as `mail-prod` |
| spec | what a target is at a revision, rendered deterministically; its digest says exactly what runs | manifest's spec, in parts: packages, software, etc, secrets |
| output | a build of a spec or of sources, content-addressed (section 7) | a root filesystem in a btrfs subvolume |
| machine | where a target runs: bare metal or a VM with A/B slots, or a container | provision's layout, ek's slots, systemd-nspawn containers |
| pool | machines, the network between them and quotas: throwaway (the lab) or a fleet (the run side) | |

### 5.2 One connector, two policies

An infrastructure connector, shared by the lab and the run side:

- **Resources:** pools, machines, targets on machines, slots.
- **Effects,** each with its recovery class (jig's `connectors.md`,
  4.3):
  - *prepare* a target at a revision into a machine's inactive slot:
    keyed by target and revision, and repeating it finds what the first
    made;
  - *activate* a slot, or *roll back* to the other: conditional on the
    active slot being the one decided from;
  - *create* a throwaway machine, keyed; *tear it down*, conditional on
    its being the one created under that key;
  - *inject a fault:* a partition, latency, a killed process, a full
    disk. Only the lab's policy grants it.
- **Facts:** each slot's revision and digests, which slot is active,
  drift, and health from the observability connector.
- **Events:** prepared, activated, fell back, drifted.
- **Holds:** a task holds the machine it changes; a held machine is
  pinned against convergence (section 9).
- **The lab and the run side differ only in pools and policy.** Rollouts
  are the run side's procedures on top; experiments are the lab's.

### 5.3 What the ground layer gives

- **The lab tests a variant, and the gap is measured.** `mail-lab` is
  mail's base with a thin lab variant: fake secrets, test certificates,
  lab addresses. os's composition of a base and a variant gives this
  already. What differs between the lab and production variants is
  never tested, so variants stay thin, and a diff of the two specs shows
  the gap: a change that touches only production's part is flagged.
- **Faults live in the pool, not in the machine.** A lab pool's host owns
  the network between its containers and their processes, so faults are
  its effects, and the system under test needs no cooperation.
- **Specs name secrets but never hold them.** A spec lists secret paths
  (os's `deploy` reference, the secrets phase), so specs can live in git
  and in the lab. A machine fetches its secrets when it prepares, under
  its own identity, from the store of the plane it belongs to. Lab
  machines have no identity in the run side's store.
- **Clusters.** A revision with several hosts maps to several targets.
  Preparing them is per target; activating them together is a decision:
  all at once in the lab, one machine at a time in production.

## 6. Pay only for what changed

### 6.1 The rule

**Every piece of work is keyed by a digest of its inputs, split into
parts, and redone only for the parts whose digest changed. Patching is
the default; building from scratch is the fallback, and the proof.**

Always the faster way, with caches wherever they help.

### 6.2 A one-line postfix change

1. **Fetch:** each machine fetches the commit. The commit's delta is all
   that moves.
2. **Render, on the machine:** templates are rendered again; the package
   resolution is reused, its input hash unchanged. Only the etc part's
   digest changes. Every other target's digests are unchanged, and those
   machines stop here.
3. **Prove:** the lab reruns only the experiments keyed by mail's
   digests. A verdict whose key is unchanged carries over to the new
   commit without running anything (section 8).
4. **Prepare:** the inactive slot is a copy-on-write snapshot of the
   active one with the new etc tree applied. No apt, no downloads.
5. **Activate:** the cheapest switch that covers what changed (6.3).

No image moves anywhere: machines receive intent, not bytes.

### 6.3 Activation has modes too

- **etc or software changed:** systemd's soft reboot into the new slot.
  Userspace restarts on the new root; the kernel stays.
- **The kernel, the initrd or boot changed:** a full reboot.
- **A container:** restarted on its new root.
- **Later, if wanted:** applying live, where a task declares how its
  files apply live (`postfix reload`). It is the fastest, and it changes
  the running slot, so the other slot stays as the rollback.

Caution follows what changed too: a one-file etc change to one service
bakes for minutes, a kernel bump for a day. That is policy, as data.

### 6.4 Patch and scratch, checked to agree

- **The risk:** a patched slot carries history a fresh build would not,
  such as a purged package's leftovers.
- **Before activation,** each prepared slot is checked against its spec
  (os's `drift`), which is cheap.
- **On a period,** the lab builds from scratch and compares with the
  patched result. A difference is a finding: a backend's bug, or a task
  that is not idempotent.
- **Scratch is chosen** when there is no base, the suite or the
  architecture changed, a patch failed, or a periodic refresh is due.
  The daemon decides from the parts' digests: mechanics, never an LLM.

### 6.5 Caches

- **Packages:** apt-cacher-ng on each machine, or on a pool's host,
  shared by its containers.
- **Software and artifacts:** content-addressed by digest.
- **Bases:** os's pristine snapshots, so a rebuild skips the cold
  bootstrap.
- **Lab clusters:** one build per spec on a pool's host, then a
  copy-on-write snapshot per container. A five-node cluster costs one
  build.
- **Compilation:** caches keyed by content, kept on the build
  environment's persistent volume (section 7).

## 7. The build daemon

One daemon, on every machine, every pool host and the lab's builders.
It knows nothing of jig, temper or the lab; to them it is a system
behind the infrastructure connector.

```
recipe     rootfs:    a target at a revision                     → a root filesystem
           artifact:  a build environment, sources, a command    → files
output     content-addressed; an output may name others by digest (a rootfs's software pins an artifact)
modes      patch from the closest earlier output (snapshot and reconcile, incremental compile),
           or from scratch; the daemon chooses
caches     packages, compilation, git objects, earlier outputs
slots      on a machine that runs what it builds: activate an output, soft reboot or full reboot
reports    each output's input and output digests, part by part; slots; drift
serves     outputs to peers by digest
```

- **A build environment is a rootfs output.** `stoa-build-env` is made
  from stoa's IaC with a pinned toolchain; the artifact recipe runs the
  build command in a container of it. A change to the toolchain changes
  the environment's digest and invalidates exactly the artifacts built
  with it.
- **Why not jobs in jig.** A job executor (a command on a host with no
  LLM) would touch the core's lifecycle and authority, the fleet, the
  worker host, the channel and every application, and would leave the
  build environment to whatever a worker has installed. The daemon
  touches one effect kind on the lab's connector ("build these inputs",
  keyed by their digest) and one fact ("inputs to an output's digest").
  It is built, tested and replaced alone: anything that answers "inputs
  to an output's digest" can stand in for it, from CI behind an adapter
  to Nix.
- **The engine decides; systems do the heavy work.** This is jig's
  split, kept: builds live behind connectors.
- **os as the backend.** os's `manifest`, `deploy`, `provision` and `ek`
  are the rootfs recipe and the slots today; the daemon is how os is
  used remotely. Its interface is the generic part, so another backend
  can stand behind it.
- **Reproducible builds help:** if an artifact depends only on its
  inputs, the lab knows before compiling whether it has it already.
  Without them, a build is made once per inputs' key and reused.

## 8. The lab

### 8.1 Names

```
experiment   data in the product's repository: a cluster (targets with a lab variant), a workload,
             faults, a checker, a cost class
subject      the digests an experiment uses: its cluster's specs, the product's artifact
key          the experiment's digest and its subject's digests
run          one execution of a key with a seed, on a pool: provision, drive, inject, collect, check
evidence     what the runs of a key showed: how many, which failed, seeds, histories, logs, metrics
verdict      the evidence judged by a policy: pass, fail, flaky, or not enough evidence yet
finding      a failure with a signature: reproduced, bisected, filed as an issue, checked again
baseline     for measurements: main's distribution, per experiment and metric
```

### 8.2 Ideas

1. **A verdict is evidence about a key, not one run.** Each use asks
   for its level: a gate three seeds of a chaos test, a nightly run more
   seeds on the same key. A commit whose keys are unchanged gets its
   statuses by lookup. Evidence only grows, so coverage deepens while
   nothing changes.
2. **Digests select what runs; judgement adds to it.** A changed digest
   selects the experiments that use it, with no LLM. An explorer reads
   the diff and adds experiments aimed at it.
3. **Seeded, as the worlds are** (`testing-strategy.md`). A fault
   schedule is drawn from a seed; a failure is a key and a seed; running
   it again measures how flaky it is. The finding's fix commits the
   experiment and its seed to the product's repository, as a
   regression test.
4. **Everything in a run is a target.** A product's workload client and
   any checker of its own run as hosts of the cluster, from the
   product's repository. Product code never runs in the lab's engine;
   the lab's own code stays generic: pools, faults, drivers, and
   checkers for crashes, a history's consistency and thresholds.
5. **Measurements compare.** Benchmarks run on quiet pools, the
   candidate and the baseline interleaved on the same machine to cancel
   noise. A result is a distribution against the baseline's; a
   regression is a finding.
6. **Bisecting is cheap,** by section 6: each step is a patch, and a key
   already proven is a lookup. It is mechanics, with no LLM.
7. **Scheduling follows value per cost:** gates that hold a landing,
   then landings, nightly runs, exploration. A burst of heads coalesces
   to its last, except for gates. Pools' slots are pooled holds (jig's
   `tasks.md`, section 6), runs are priced, budgets are per period.
   Untrusted heads wait for a maintainer, and pools reach nothing but
   their caches.

### 8.3 Agents

In the lab's engine, with reads of evidence and experiments to run as
their tools:

- **explorer:** aims new experiments at what changed;
- **triager:** matches a failure's signature against open findings, and
  classes it;
- **analyst:** explains a regression, for example by diffing profiles.

The lab never writes the product's repository: a finding carries its
reproducer, and temper commits it.

### 8.4 On jig

- **A run** is a task whose executor is a procedure (provision, drive,
  collect, check), holding pool slots, priced.
- **Findings and baselines** are the lab's own child domains; findings
  are projected as issues.
- **Connectors:** infrastructure and observability, shared with the run
  side; the forge, to read repositories, post statuses and file issues;
  drivers (SMTP, IMAP, HTTP, DNS) for agents exploring by hand.

### 8.5 A head of stoa

```
head h: raft.rs changed; its change's gate asks for lab/chaos-smoke
 │
 ├─ 1. digest stoa's build inputs at h: S_h. Built before? no
 ├─ 2. artifact recipe in stoa-build-env, its compile cache warm: only the touched crate
 │     recompiles → A_h, under (S_h, the environment's digest)
 ├─ 3. cluster stoa-1..3: the lab variant, stoa pinned to A_h. Only the software part changed
 │     since the last head, so each slot is patched: one binary swapped
 └─ 4. chaos-smoke, seeds 1–3 → evidence under its key → status at h
```

The experiment's cluster names "stoa, built from this head", not a fixed
digest; the lab resolves it to A_h before rendering. A later head h′
that edits only the README keeps S, so A_h, the specs and the keys are
unchanged, and its statuses are posted by lookup: nothing built, nothing
run.

### 8.6 Postfix

The same model: the subject is `mail-lab`'s spec. Experiments drive the
service from a client host in the cluster: a message sent and
delivered, its DKIM signature verified, TLS offered, no open relay. They
gate changes on the IaC repository before landing.

## 9. The run side

A sketch, still to be designed:

- **Convergence:** a standing procedure brings each target to the IaC
  repository's main. Only machines whose specs changed do anything
  (section 6).
- **Rollouts:** order, canaries and bake times as policy, with caution
  following what changed (6.3); each step needs the observability
  connector's verdict on health.
- **Health at commits:** posted as statuses, for the goals that wait on
  them (section 4).
- **Incidents:** observability's alerts, triaged; an incident lead with
  reads of both connectors; people on call accepting what policy
  reserves to them (jig's `examples.md`, section 7, is its sketch).
- **Rollback is a slot switch,** cheap and conditional, and may be
  allowed ahead of time. It holds the machines against convergence
  until the desired state matches again: the run side files an issue,
  temper lands a revert or a fix, and the hold is released.
- **No hand on a machine.** Agents read; their only writes are the
  infrastructure connector's effects. Any other fix is a change.
- **Bootstrap:** the run side's first machine is made by hand with os's
  `provision`; after that it updates itself through its own slots,
  which fall back on their own when a new one does not come back
  healthy.

## 10. A story: DKIM for the mail server

1. A person asks a chat in temper for DKIM. It becomes a goal and a
   change on `mail-iac`.
2. The change's gate needs `lab/mail` at its head. The lab prepares
   `mail-lab` at that revision, a patch of its last slot, and drives it
   over SMTP: a message goes out signed, the signature verifies, the
   relay is still closed. The status is posted, and the change lands.
3. The run side converges staging, then one production machine, then
   the rest, each by a prepared slot and a soft reboot. After a day of
   normal deliverability it posts `run/prod: healthy` at the commit.
4. temper's goal, waiting on that status, ends.
5. A week later bounces climb. The run side triages, switches the
   machines back to their previous slot, holds them, and files an
   issue. temper lands a revert or a fix; the hold is released and the
   machines converge.

## 11. What belongs to what

```
skein          io and protocol kit
smith          the agent
jig            the engine kit: primitives and promises
connectors     crates on jig, shared by applications: forge, infrastructure, observability, drivers
temper         write: forge, workers, coder and reviewer charters
the lab        prove: experiments, evidence, findings, baselines; explorer, triager, analyst
the run side   run: convergence, rollouts, holds, incidents; triager, incident lead
the daemon     outputs from inputs, slots; os its first backend
ops            jig's example, on fakes; its connectors may seed the real ones
```

- **Applications, or deployments?** The lean is three applications on
  shared connector crates. Each is useful alone: a team can run postfix
  with the run side and edit its IaC by hand; a library needs no run
  side; the lab serves anyone on the forge. They differ in trust, in
  reliability, and in their own objects (experiments and baselines;
  rollouts and incidents). Independence within one plane, such as a lab
  that proves temper itself, comes from a deployment pinned to a
  trusted release.
- **What it costs:** a goal's tree stops at its plane, joined to the
  others by facts at commits and issues; each plane has its own budget;
  a person has an inbox per plane until clients are merged.
- **jig's own example** was chosen to share nothing with temper, and it
  is a sketch of the run side. jig would gain real second and third
  applications.

## 12. What it asks

- **jig:**
  - connector crates shared by applications (`qa.md`, section 9);
  - clients that are not people, for the active lab;
  - services as parties on the forge.
- **temper:** gates that require another party's status at a head;
  goals that wait on a status at a landed commit; a lab connector,
  later.
- **os, or the backend behind the daemon:**
  - each part's digest reported upward;
  - slots made by snapshot;
  - soft-reboot activation;
  - boots counted, so a slot that does not come back healthy falls
    back on its own;
  - the artifact recipe;
  - a remote interface: the daemon.

## 13. What this changes in `qa.md`

- **Where QA lives:** in the lab, an application of its own, not in a
  temper deployment of its own. temper is one of the lab's subjects; a
  lab deployment pinned to a trusted release proves it.
- **Environments:** the infrastructure connector of section 5, shared
  with the run side, in place of a connector of temper's.
- **Temper in a box:** an experiment in temper's repository, its cluster
  a throwaway forge, an engine, a worker and a scripted LLM endpoint.
- **Drivers:** connectors' calls for agents, and hosts of the cluster for
  scripted workloads.
- **Oracles:** checkers, generic or the product's.
- **Findings:** the lab's, keyed by signature, reproduced from a key and
  a seed.
- **Unchanged:** its aim past the worlds, its loop back into them, its
  order of building from no agent upward.

## 14. Open questions

- **Applications:** three, as 11 leans, or fewer; and their names.
- **Desired state:** whether the run side ever writes it, for a quick
  revert during an incident, or always asks temper.
- **Drivers:** connectors' calls, bounded by authority, or a shell in a
  sandbox beside the target, more flexible and not bounded.
- **Evidence for a pass:** how many seeds a gate on a nondeterministic
  experiment needs.
- **Who proves the lab:** another lab deployment, pinned to a release.
- **Outputs beyond their builder:** peers fetching from each other
  first; a store per pool or per site later, keyed the same way.
- **Applying live:** whether it is worth the slot it modifies.
- **The daemon's interface:** what is generic, so that os is one backend
  among others.
- **Secrets:** the run side's store, and how machines prove who they are
  to it.
