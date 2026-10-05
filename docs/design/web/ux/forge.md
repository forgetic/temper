# The forge's panels

Provisional, 2026-10-05. What the forge connector contributes to the
web: a change's panel, a project's changes and landing queues, a
person's approval at a head, repositories' adoption and health, drift
explained, and goals as the forge shows them. The core pages know none
of it (README.md, 5.1); each panel shows what `domain/forge.md` keeps or
reads, and links to the forge for code. What is still open is listed in
section 10.

## 1. In one page

- **A change is shown as its way to landing,** one step at a time, each
  with its reason in words: CI running, a review asked for, second in
  the queue, updating from main, resolving a conflict.
- **Queues are shown, not managed.** A landing branch's queue is its
  ready changes in order; a person changes the order through goals'
  priorities (projects.md, 3.2), never by moving changes about.
- **Code stays on the forge.** Panels link to a pull request's diff, its
  files and its failing jobs; the web shows states and verdicts, not
  code.
- **Approvals are at a head:** a person approves exactly the head they
  were shown, and the panel says when a later head still carries it.
- **What the forge did not tell temper is said:** a repository with no
  webhooks, protection temper could not read, a branch someone else
  moved.

## 2. A change's panel

On a change task's page (tasks.md, 5.3) and, folded to a card, wherever
the change is named: a chat that handed it a branch, a goal's plan, the
inbox.

### 2.1 Its way to landing

The steps of `domain/forge.md`, 8.2, with the one it is at and why:

```
producing ─ opening ─ checking ─ gating ─ queued ─ first ─ landing ─ landed
                         repairing · updating · resolving
```

| Step | It says |
|---|---|
| producing | T21 producing it, as a chip |
| opening | opening its pull request |
| checking | CI on its head, and for how long |
| gating | which gates are running, as chips |
| queued | its place in its landing branch's queue, and who is ahead |
| first | first in the queue: updating from its base, CI on the updated head, or merging |
| repairing | why (CI's failure, a gate's remarks, a semantic conflict), the repair as a chip, and how many repairs are left |
| updating | the base it takes in |
| resolving | the conflicting files, the resolution as a chip, and how many are left |
| landed | its merge commit, and when |

### 2.2 What it shows

- **Its pull request:** number, title, head and base, state,
  mergeability; links to it, its diff and its files on the forge.
- **CI on its head:** the combined state and each context's, with a link
  to a failing job, and the part of its output a repair was given, cut
  as the repair got it.
- **Its gates** (`domain/forge.md`, 8.3): each with its kind (a review by
  a lens, a person's approval, checks), blocking or advisory, who asked
  for it (the change, the project's landing rule, its plan), its verdict
  and the head it was given at, and whether it carries over to the
  current head; the task running it, as a chip. An advisory gate that
  failed is reported and holds nothing, and the panel says so.
- **Its heads,** in order, each with what made it (produced, a repair, a
  clean update, a resolution) and its CI and verdicts, so the person can
  see why a gate is asked again: a repair's head asks every gate again,
  a clean update's carries them.
- **Its bounds:** repairs and resolutions used of their limits; each wait
  against its stall: "CI has not reported for 25 minutes; held at 60".
- **Its delegates:** producing, repairs, reviews, resolutions, as its
  plan (tasks.md, 6.1).

### 2.3 What may be done

- **Amend its gates:** add a review by a lens, or a person's approval, or
  drop one the plan asked for; a gate added while it is queued takes it
  out of the queue until its verdict, which the form says.
- **Release** it, held (section 6; tasks.md, section 7).
- **Cancel** it: its pull request closed with a comment saying why, its
  branch deleted, its commits still readable under the closed pull
  request; a merge already sent may still land, and then it ends landed
  (`domain/forge.md`, 8.6).

## 3. Changes and landing queues

A project's changes, by landing branch (`domain/forge.md`, section 9):

- **The queue:** the ready changes in order, each with its goal and
  priority and when it became ready; the first says what it is doing
  (updating, CI on its updated head, merging); a change past the aging
  window is marked as going ahead of those ready after it.
- **The order's rule,** in a line: "by goal priority, then by when each
  became ready; ready for over two hours goes first".
- **Not ready yet,** by step: producing, checking, gating, repairing,
  resolving.
- **Held,** each with its reason.
- **The branch:** its tip, CI on it, and the landings recently made,
  paged.
- **Paused:** when CI fails on the branch's tip, "main fails at its tip
  since 10:02; T88 repairs it, first in the queue; the rest wait", with
  the repair as a chip. Past its repairs' bound, the queue is held for a
  person, whose card offers to release it as it is (a flaky check) or to
  cancel the repair.
- **Where temper may not merge,** the queue is still ordered for people
  to see, each ready change "waiting for someone else's merge".

## 4. Approvals

A person's approval is a person task at an exact head (inbox.md, 4.4;
`domain/forge.md`, 8.3):

- **what is asked:** approve this change at this head, for the role and
  rule that asked for it;
- **the change at that head:** its pull request, CI at the head, the
  other gates' verdicts, what changed since an earlier approval if
  there was one, and a link to its diff on the forge;
- **the answer:** approve, or ask for changes with remarks, which become
  the brief of the repair that follows.

If the change's head moves while the card is open, the card says so. A
clean update temper made carries the approval, unless the project's rule
asks for the exact head; any other new head asks again, as a new person
task at that head, and the old card says it has been replaced.

## 5. Repositories

### 5.1 Adopting one

An owner adopts a repository into a project (`domain/forge.md`,
section 4):

- **which:** a forge, a repository;
- **its role:** owned, where temper pushes and lands; a fork of an
  upstream; or context, read only;
- **what the project names there:** its landing branches, temper's
  branch prefix (`temper/` by default), its merge style (the
  repository's own, squash where it has none), and how its changes are
  checked without CI.

Before the adoption is confirmed, the web shows what temper found:
temper's permission there, whether it may merge, the protection on the
landing branches and on its prefix (or that it could not read it), its
webhooks or none, CI or none, and the ceiling that follows, in words. A
repository where another deployment's branches already live under the
prefix is refused, saying so (`domain/connectors.md`, section 11).

### 5.2 Health

Wherever a repository is shown (the project's page, its settings, the
system's page):

- **what temper may do** there, and whether that narrowed since adoption
  ("a merge was refused: temper may no longer land into main");
- **its webhooks:** when they were last heard; none, or none for days,
  means changes are seen by polling alone, more slowly;
- **its landing branches:** their tips and CI on them;
- **what is held** on it for drift (section 6).

## 6. Drift, explained

A held change's card for drift (`domain/forge.md`, 8.5): what changed on
the forge, by whom and when, a link to it there, and what each action
will do. Nothing is written over until the person decides.

| What changed | Release | Otherwise |
|---|---|---|
| its pull request closed by someone else | reopens it | cancel: the change closes |
| its branch moved by another identity, backwards, or while nothing of temper's wrote it | takes the new head as the change's, checks it and asks every gate again | cancel |
| its branch deleted | creates it again at the head temper last knew, and reopens the pull request | cancel |
| its pull request retargeted | retargets it back | amend its landing branch to the new base, then release; cancel |
| merged by someone else | not held: it is landed, like any landing | |

## 7. Goals on the forge

- **A goal's issue,** in its project's home repository
  (`domain/forge.md`, section 12): the goal's page links to it, says when
  it was last written, and that it is a projection: what people write
  there is not read, and the web is where temper is reached.
- **Pull requests name their goal's issue,** so a person reading the
  forge finds the goal, and from it the web.
- **Landings as news,** on a goal's page (projects.md, 3.3): each
  landing in its repositories, marked as its own, overlapping its work
  (it woke the goal), not overlapping (kept), or unknown, as for a
  landing too large to compare (it woke the goal).
- **Findings,** the issues agents filed, are on the project's page
  (projects.md, section 4).

## 8. Taking part, later

Not needed first (`domain/forge.md`, sections 13 and 14), and kept
possible with what is above:

- **A participating object** (an outside contribution, an issue others
  opened) shows on the page of the task dealing with it: what others
  wrote there, as the news it heard, and what temper would write, a
  reply or a review, as a proposal card with its exact text, when the
  project's policy has a person accept it first.
- **Where temper may not merge,** a ready change waits for a maintainer's
  merge, and people's reviews on its pull request show as its news.
- **An upstream submission** is a proposal a person accepts before
  anything is posted, showing exactly what will be.

## 9. From the domain

| The web | The domain |
|---|---|
| a change's way to landing, its gates and heads | the change procedure (`domain/forge.md`, section 8) |
| queues, paused and held | landing queues (`domain/forge.md`, section 9) |
| approvals | gates (`domain/forge.md`, 8.3); person tasks (`domain/people.md`, section 8) |
| adoption and health | `domain/forge.md`, sections 4 and 5; `domain/connectors.md`, section 11 |
| drift | `domain/forge.md`, 8.5; `domain/connectors.md`, section 10 |
| goals' issues, landings as news | `domain/forge.md`, sections 7 and 12 |

## 10. Open questions

- **A change's files in the web:** whether the panel lists the files a
  change touches, from the connector's read, short of showing the diff.
- **Conflicts seen early** (`domain/forge.md`, 20.2): warning two goals
  whose open changes conflict before either lands, as a panel on both.
- **A queue's throughput:** if landings are ever batched, how a batch and
  its bisection show.
- **A person's approval as a rule** posted as the person
  (`domain/people.md`, section 14), which the approval card would then
  say.
