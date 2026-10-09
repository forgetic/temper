# Where the inline agent lives

Draft, 2026-10-07. A question from a design discussion on 2026-10-07,
after jig adopted one host for its runs (`one-host.md`): whether jig
needs `jig-inline-agent`, and if not, what takes its place. Three
options, none adopted: jig's `hosts.md` describes what is designed.
smith's draft `one-host.md` (smith's side of jig's one host) already
argues the first. Section 6 is a recommendation; section 7 lists what is
open.

## 1. In one page

- **What the inline agent is:** what was left of `jig-local-host` once
  its run lifecycle moved into the hub, `jig-host`. The hub never runs an
  agent; it hands each run to an agent capability. On a worker, that is
  smith's host domain, one process per run. In the engine, something has
  to run smith's domain in memory and speak the same vocabulary: the
  inline agent (jig's `hosts.md`, 5.2). It is built, as
  `jig-inline-agent` (55c95516).
- **It is not specific to jig.** It composes smith's domains and passes
  their requests up; it knows nothing of jig's core. smith's local host
  needs the same thing for its one-process form (smith's `host.md`,
  section 9).
- **The options:**
  1. **Move it to smith,** as the twin of `smith-host-domain`, used by
     jig's engine and smith's local host alike;
  2. **Make it a mode of `smith-host-domain`:** one child, which runs a
     run as a process or in memory;
  3. **No agents in memory:** the engine runs its agents as processes,
     through `smith-host-domain`, and nobody needs an inline agent for
     jig.
- **Recommendation** (section 6): the first, and record the third as a
  composition an application may choose, which one host already allows.

## 2. Option 1: move it to smith

smith's draft `one-host.md` sets this out: a crate beside
`smith-host-domain`, such as `smith-host-inline`, sharing its vocabulary,
composed by jig's engine and by smith's local host.

- **For:**
  - the one-process form is written and tested once, by the project
    that owns smith's domain;
  - jig links smith in two places, not three: the charter's translation
    and the client's view of conversations;
  - jig's engine composes it as it composes any of smith's crates, and
    jig retires `jig-inline-agent` at a repin of smith.
- **Against:**
  - jig waits on smith's pass, or keeps its own crate until it lands;
  - a change jig needs in it is a change to smith first.

## 3. Option 2: a mode of `smith-host-domain`

One child, with two ways of running a run behind one boundary.

- **For:**
  - one crate and one boundary type, so the two kinds cannot drift
    apart;
  - a host could mix the two kinds per run without composing two
    children.
- **Against:**
  - it mixes two different jobs: supervising processes (spawn, channel
    rules, watchdog, terminate and kill, proof of an empty tree) and
    composing domains in memory, which share only their vocabulary;
  - a host that never spawns a process still carries the supervision,
    and one that never composes a domain still links `smith-domain`;
  - its worst case and its world become the sum of both.

smith's draft lists the same choice as open ("one crate or two"); a
shared vocabulary crate under two children gives option 2's first
benefit without its costs.

## 4. Option 3: no agents in memory

The engine's root composes `smith-host-domain` for the engine's slots,
as a worker's root does, and no workspace: each run on the engine is an
agent process on the engine's machine.

- **For:**
  - jig needs no inline agent, and neither does any application's
    engine;
  - the engine's agents get a process as their containment boundary;
  - two costs of agents in the engine go (jig's `hosts.md`, section 3):
    the engine's worst case no longer includes its runs, and LLM traffic
    leaves the engine's loop, made by each agent process with its
    grants;
  - the engine's root and a worker's compose the same parts, the link to
    the core and the workspace aside.
- **Against:**
  - a process per run, even for a chat: a spawn per start, and an
    agent's memory per process;
  - the engine's deployment ships smith's agent beside the engine, or
    the engine's binary runs as the agent too;
  - LLM credentials leave the engine's process, as grants on each
    agent's channel, as they do on workers;
  - the engine's agent processes must end with the engine, which needs
    the same proof of an empty tree as a worker's;
  - smith still wants an inline agent for its local host, tests and
    development (smith's `host.md`, section 9), so the code moves out of
    jig but does not vanish.

## 5. What one host already allows

Both kinds of agent speak one vocabulary (jig's `hosts.md`, section 5),
and the hub does not know which it has. So option 3 needs no change to
jig's mechanism: an engine's root may compose `smith-host-domain` for its
slots instead of an inline agent today. What jig decides is only:

- whether its design offers agents in memory in the engine at all, or
  processes only;
- if it offers them, who owns the inline agent (options 1 and 2).

## 6. Recommendation

- **Own it in smith,** as option 1 and smith's draft say, with a shared
  vocabulary crate if the two children's boundaries would otherwise
  repeat each other. jig keeps `jig-inline-agent` until smith's lands,
  then retires it at a repin.
- **Allow option 3 as an application's composition,** written into jig's
  `hosts.md`: an engine's root composes either kind for its slots. In
  memory suits chats and triage, many and cheap; processes suit an
  application that wants its engine's runs contained, or its LLM traffic
  off the engine's loop.

## 7. Open

- **The default for `ops`:** in memory, as designed, or processes, to
  exercise option 3 in jig's own example.
- **When jig switches** to smith's inline agent: as soon as smith has
  it, or at jig's next repin after both plans finish (smith's draft
  asks the same).
- **The engine's agent processes and restarts:** whether the engine's
  process tree proof is the protocol layer's, as on a worker, or the
  deployment's (a cgroup, a service manager).
