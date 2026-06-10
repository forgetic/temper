# High Forgejo CPU During Idle Dogfood Workflow

Date: 2026-06-08

Repository context: local Forgejo at `http://127.0.0.1:3000`, dogfood services for `ai/smith`, `ai/temper`, `ai/jig`, and `ai/bench`.

## Executive Summary

Forgejo was not idle even though there was no active workflow work. The high CPU was driven by the dogfood mechanical workers repeatedly scanning and attempting to land already-merged pull requests.

The worst offender was `smith-mechanical.service`. It repeatedly found 26 closed/merged PRs as `landing` candidates because they still had the `implementation` label and a passing CI signal. Each attempted `land_pr` then failed with a stale/contradicted precondition because the PR was already merged. The worker treated that as unchanged and retried on the next idle poll.

The expensive part is the Forgejo Actions CI fallback. This local Forgejo does not expose Actions runs over the REST endpoint used by Temper, so Temper falls back to password-authenticated web UI scraping. For every stale PR candidate it logs into the Forgejo web UI, scrapes the repository Actions page, then POSTs each visible run's live-view JSON before filtering to the relevant PR. With many stale candidates, this becomes hundreds of Forgejo web requests per idle tick.

## Observed Symptoms

Initial `top` sample from the operator:

```text
PID 779 free ... %CPU 124.6 ... COMMAND forgejo-server
load average: 1.63, 1.77, 2.13
```

Process inspection during the investigation showed the Forgejo server had consumed more CPU time than wall time since startup:

```text
PID  ELAPSED   TIME     %CPU  COMMAND
779  05:12:57  06:15:47 120   /home/free/.local/bin/forgejo-server --config /home/free/.local/state/forgejo/custom/conf/app.ini web
```

Forgejo's request log was very large for a local idle instance:

```text
/home/free/.local/state/forgejo/log/gitea.log 224M, about 1.1M lines
/home/free/.local/state/forgejo/data/forgejo.db 42M
/home/free/.local/state/forgejo/data/actions_log 9.9M
```

Recent log traffic showed dozens of Forgejo requests per second even while there was no active CI or open PR work.

## Running Services

The local user systemd session had all dogfood services running:

```text
bench-architect.service
bench-engineer.service
bench-mechanical.service
bench-trigger.service
jig-architect.service
jig-engineer.service
jig-mechanical.service
jig-trigger.service
smith-architect.service
smith-engineer.service
smith-mechanical.service
smith-trigger.service
temper-architect.service
temper-engineer.service
temper-mechanical.service
temper-trigger.service
forgejo.service
```

The mechanical workers were configured with a short CI polling cadence:

```text
CI_STATUS_POLL_MS=1000
IDLE_POLL_MAX_MS=8000
```

These values are present in:

```text
/home/free/.config/smith/smith.env
/home/free/.config/temper/temper.env
/home/free/.config/jig/jig.env
/home/free/.config/bench/bench.env
```

The mechanical worker shims run `temper-worker` like this:

```text
--kind mechanical --poll-ms "${CI_STATUS_POLL_MS}" --idle-poll-max-ms "${IDLE_POLL_MAX_MS}"
```

## Evidence From Worker Logs

`smith-mechanical.service` repeatedly reported no progress but many stale candidates:

```text
mechanical_automation_summary candidate_count=26 changed=false applied_count=0 stale_unchanged_count=26 unchanged_count=26
```

It logged every stale PR as a `landing` candidate:

```text
{"artifact_kind":"implementation_pr","artifact_number":17,"artifact_type":"pull_request","diagnostic_classes":["contradicted_precondition"],"event":"mechanical_automation_execution","outcome":"unchanged","queue":"landing","transition":"land_pr"}
```

The same pattern repeated for many already-closed PR numbers, including:

```text
17, 20, 22, 24, 26, 28, 30, 32, 34, 36, 42, 44, 46, 48, 52, 54, 59, 61, 63, 65, 67, 69, 71, 73, 75, 77
```

Forgejo state confirmed there were no open implementation PRs for `ai/smith`, but many closed merged PRs had both `implementation` and `landed` labels. Example:

```text
PR #77 state=closed merged=true labels=[implementation, landed]
PR #75 state=closed merged=true labels=[implementation, landed]
PR #73 state=closed merged=true labels=[implementation, landed]
```

## Evidence From Forgejo Request Logs

Recent Forgejo logs showed the Smith mechanical worker connection repeatedly executing the web UI CI fallback. A representative excerpt:

```text
GET  /api/v1/repos/ai/smith/actions/runs?limit=200 -> 404
GET  /user/login -> 200
POST /user/login -> 303
GET  /ai/smith/actions -> 200
POST /ai/smith/actions/runs/40/jobs/0 -> 200
POST /ai/smith/actions/runs/39/jobs/0 -> 200
...
POST /ai/smith/actions/runs/11/jobs/0 -> 200
```

A count over a recent sample showed the same live-view endpoints being hit repeatedly:

```text
283 POST /user/login
283 POST /ai/smith/actions/runs/40/jobs/0
283 POST /ai/smith/actions/runs/39/jobs/0
283 POST /ai/smith/actions/runs/38/jobs/0
...
283 POST /ai/smith/actions/runs/12/jobs/0
```

This explains why the CPU was charged to `forgejo-server` rather than the Rust workers: the workers were mostly sleeping or doing light orchestration, while Forgejo was repeatedly rendering login/actions pages and serving live-view JSON.

## Root Cause

There are two interacting issues.

### 1. Closed/Merged PRs Remain Active Queue Candidates

The basic-delivery workflow defines the landing queue as:

```json
{
  "id": "landing",
  "artifact": "implementation_pr",
  "labels": ["implementation"],
  "condition": { "kind": "ci_passed" },
  "automation": {
    "actor": "mechanical",
    "transition": "land_pr"
  }
}
```

Merged PRs still carry the `implementation` label after landing. They also carry `landed`, but `landed` does not remove them from the `landing` queue because the queue does not exclude `landed`.

In Temper's active queue scan path:

- `crates/temper-runner/src/scan/candidate.rs` includes closed and merged PR queries for queue labels in normal and automated scans.
- `crates/temper-runner/src/scan.rs` classifies those PRs and evaluates queue conditions.
- `crates/temper-workflow/src/plan/queue.rs` matches only kind, labels, and optional condition. It does not know or check whether the Forge artifact is open, closed, or merged.

So a closed merged PR with labels `implementation, landed` and a passing CI signal still matches `landing`.

When the mechanical worker then executes `land_pr`, the executor re-reads fresh state and refuses to merge an already-merged PR. This produces `contradicted_precondition`, which the worker treats as expected stale state and retries later.

### 2. CI Signal Reads Are Very Expensive on This Forgejo Version

Temper tries the REST Actions endpoint first:

```text
GET /api/v1/repos/ai/smith/actions/runs?limit=200
```

This local Forgejo returns `404`, so Temper falls back to the web UI CI reader in `crates/temper-forge-forgejo/src/ci.rs` and `crates/temper-forge-forgejo/src/ci_ui.rs`.

The fallback currently logs in and discovers run IDs for each CI read:

1. `GET /user/login`
2. `POST /user/login`
3. `GET /{owner}/{repo}/actions`
4. For each discovered run: `POST /{owner}/{repo}/actions/runs/{run}/jobs/0`
5. Then it filters jobs to the target PR/branch/commit.

This is acceptable for a small number of live PRs, but pathological when stale closed PRs are repeatedly treated as active landing candidates.

## Secondary Contributors

- The mechanical idle maximum poll interval is only 8 seconds. Once stale candidates exist, the hot path runs roughly every idle tick.
- There are four dogfood repositories running mechanical workers. Smith dominates due to 26 stale landing candidates, but Jig and Temper showed the same class of stale candidate behavior with fewer items.
- The Forgejo server was constrained with `FORGEJO_GOMAXPROCS=2`, which limits host saturation but does not reduce total unnecessary work. It can also make each burst last longer.
- Forgejo router logging at `info` records every request, adding I/O and producing a very large `gitea.log` while this loop is active.

## Immediate Mitigations

### Stop Idle Mechanical Workers

If no landing activity is needed, stop mechanical workers temporarily:

```sh
systemctl --user stop smith-mechanical.service temper-mechanical.service jig-mechanical.service bench-mechanical.service
```

This should make Forgejo mostly idle. Role workers can remain running if desired; they poll much less frequently and were not the main source of traffic.

### Raise Mechanical Idle Cadence

Increase the mechanical polling intervals in each repo env file:

```text
CI_STATUS_POLL_MS=30000
IDLE_POLL_MAX_MS=300000
```

Then restart the mechanical services:

```sh
systemctl --user restart smith-mechanical.service temper-mechanical.service jig-mechanical.service bench-mechanical.service
```

This does not fix the stale candidate bug, but reduces idle load by roughly one to two orders of magnitude. Webhooks still wake the workers on relevant events.

### Remove Active Landing Labels From Merged PRs

A tactical cleanup is to remove the `implementation` label from already-merged PRs, or otherwise change labels so merged PRs no longer satisfy the active `landing` queue.

Do this carefully because `implementation` is also an artifact identity label in the current workflow. Removing it may make historical PRs less classifiable as implementation PRs. A better long-term approach is to introduce a separate active queue label.

## Durable Fix Recommendations

### 1. Do Not Include Terminal Artifacts in Active Queue Scans

Normal role scans and automated scans should not list closed issues or closed/merged PRs as active queue candidates. Closed/merged history belongs in audit and reconciliation paths, not in active work queues.

Likely code area:

```text
crates/temper-runner/src/scan/candidate.rs
```

Current behavior:

- `CandidateQueryBuilder::add_queue_interest` adds both open and closed interest for all scan modes.
- For PRs, closed interest emits both `PullRequestState::Closed` and `PullRequestState::Merged` queries.
- `scan_automated_queues` uses `ScanMode::Automated`, but still receives terminal PR queries for non-empty queue labels.

Recommended behavior:

- For `ScanMode::Normal` and `ScanMode::Automated`, queue interest should query open artifacts only.
- For `ScanMode::Audit`, include terminal workflow-labelled/recoverable candidates.
- Keep bounded reconciliation's terminal candidate logic separate.

This directly removes already-merged PRs from the `landing` hot path.

### 2. Represent Forge Terminal State in Classified Artifacts or Scan Filtering

The workflow matcher currently cannot distinguish open from closed/merged because `ClassifiedArtifact` does not carry provider state. This makes it easy for terminal artifacts to match active queues if they are listed.

Options:

- Prefer scan-level filtering: active scans only list open artifacts, audits list terminal artifacts.
- Or add terminal/open state to `ClassifiedArtifact` and make active queue matching reject terminal artifacts by default unless a queue explicitly opts into terminal history.

The scan-level fix is simpler and aligns with the existing docs in `docs/reference/production-worker.md`, which already say active candidate queries should avoid pure terminal identity history.

### 3. Split Identity Labels From Active Queue Labels

The current `implementation` label is both:

- artifact identity: identifies a PR as an `implementation_pr`
- active queue selector: selects PRs for `landing`

That dual use makes terminal history look active forever.

Recommended workflow change:

- Keep `implementation` as identity.
- Add a separate active state/queue label such as `landing` or `landable`.
- Add it when a PR is ready for mechanical landing.
- Remove it in `land_pr` after merge.
- Make the landing queue require `landing`, not just `implementation`.

Example shape:

```json
{
  "id": "landing",
  "artifact": "implementation_pr",
  "labels": ["landing"],
  "condition": { "kind": "ci_passed" },
  "automation": { "actor": "mechanical", "transition": "land_pr" }
}
```

This is more explicit and makes closed PR labels less dangerous.

### 4. Cache Web UI CI Reads Per Repo Tick

The web UI fallback should avoid repeating login, actions-page discovery, and live-view POSTs for every PR candidate in the same repo/tick.

Possible improvements:

- Reuse the web UI session cookie within one worker tick.
- Cache discovered run IDs and live-view JSON per repo for the duration of a tick.
- Read `/actions` once per repo/tick, not once per candidate.
- If possible, narrow run discovery by branch or commit before reading every run.

This would reduce the cost of a burst from roughly `candidate_count * visible_run_count` to roughly `visible_run_count`.

### 5. Use Commit Status as a Cheap Gate When Available

The Temper test comments suggest the Forgejo commit status API can report runner verdicts by commit SHA. If the local Forgejo exposes reliable commit statuses, use that as the first CI gate check before falling back to web UI live-view scraping.

Potential endpoint shape:

```text
GET /api/v1/repos/{owner}/{repo}/commits/{sha}/status
```

If this works for PR head SHAs, it would be much cheaper than scraping Actions HTML and live JSON.

### 6. Reduce Web UI Login Frequency

Even without full CI caching, avoid logging into Forgejo for every single CI read. A `WebUiClient` session should be reused until a login bounce occurs.

Current pattern observed in logs:

```text
GET /user/login
POST /user/login
GET /actions
POST /runs/.../jobs/0
```

This repeated hundreds of times. Session reuse would eliminate much of the CPU and log churn.

### 7. Consider Lower-Volume Forgejo Logging for Local Dogfood

Once the functional loop is fixed, consider reducing Forgejo router log volume or configuring log rotation. This is not the root cause, but the current loop produced a 224 MB request log.

## How to Confirm a Fix

After applying the active-scan or workflow-label fix:

1. Restart the mechanical workers.
2. Watch `smith-mechanical.service` logs:

```sh
journalctl --user -u smith-mechanical.service --since "2 minutes ago" --no-pager
```

Expected healthy idle behavior:

```text
mechanical_automation_summary candidate_count=0 changed=false applied_count=0
```

For Smith specifically, `candidate_count` should not be 26 while there are no open PRs.

3. Check recent Forgejo request rates:

```sh
tail -n 5000 /home/free/.local/state/forgejo/log/gitea.log \
  | awk '/router: completed/ {sec=$1" "$2; cnt[sec]++} END{for (s in cnt) print cnt[s], s}' \
  | sort -k2 \
  | tail -20
```

Expected healthy idle behavior: near-zero request volume apart from occasional role polls, runner task fetches, or browser activity.

4. Check for absence of repeated web UI CI fallback bursts:

```sh
tail -n 10000 /home/free/.local/state/forgejo/log/gitea.log \
  | grep -E 'POST /user/login|/actions/runs/.*/jobs/0' \
  | tail
```

Expected healthy idle behavior: no continuous stream of login and live-view POST requests.

5. Check CPU:

```sh
ps -p 779 -o pid,etime,time,%cpu,%mem,args
```

Expected healthy idle behavior: low current CPU for `forgejo-server` outside actual workflow or browser activity.

## Suggested Implementation Priority

1. Fix active queue scans to exclude terminal artifacts for normal and automated scans.
2. Add tests proving merged PRs with `implementation` and passing CI are not emitted by `scan_automated_queues` for `landing`.
3. Introduce a separate active landing label in the workflow, if the workflow model wants closed implementation PRs to retain the identity label for history.
4. Cache Forgejo web UI CI reads per repo/tick.
5. Optionally add commit-status based CI checks before web UI fallback.
6. Tune mechanical poll defaults upward for local dogfood deployments.

## Files and Code Areas To Review

```text
/home/free/src/rust/temper/crates/temper-runner/src/scan/candidate.rs
/home/free/src/rust/temper/crates/temper-runner/src/scan.rs
/home/free/src/rust/temper/crates/temper-workflow/src/plan/queue.rs
/home/free/src/rust/temper/crates/temper-forge-forgejo/src/ci.rs
/home/free/src/rust/temper/crates/temper-forge-forgejo/src/ci_ui.rs
/home/free/src/rust/temper/docs/reference/production-worker.md
/home/free/src/rust/temper/crates/temper-workflow/fixtures/basic-delivery.json
```

## Final Assessment

The immediate cause of high Forgejo CPU is not live CI. It is an idle-loop bug where mechanical workers repeatedly treat terminal, already-landed PRs as active landing candidates. The CI web UI fallback then multiplies that stale candidate set into a large number of Forgejo requests. Fixing active scans so terminal artifacts are excluded from normal automated queues should remove the main CPU load. Caching or replacing the web UI CI fallback will make the system much more robust when legitimate active PRs exist.
