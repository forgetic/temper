# Migrate a persistent Forgejo service to 16.0.1

Forgejo 16.0.1 is Temper's minimum supported Forgejo release. This runbook
records the deployment boundary for replacing password-authenticated CI scraping
with token-authenticated Actions run/job APIs. It does **not** automate a Forgejo
migration. An operator owns every backup, rehearsal, service change, and
production verification step.

The required CI endpoint is:

```text
GET /api/v1/repos/{owner}/{repo}/actions/runs/{provider_run_id}/jobs
```

`provider_run_id` is the provider database id from Actions run discovery, not a
repository-local display number. Requests use the same Forge API token as other
Temper reads. The API-only binary has no login, HTML, live-view, repository-wide
tasks, or password fallback.

## Required order

Do not reorder these phases:

1. **Prove the Bench fixture.** Use the merged `ai/bench` fixture revision pinned
   by this repository. Run the Forgejo backend live contract against its
   checksum-verified Forgejo 16.0.1 server and host-mode runner. The contract
   must observe one successful and one intentionally failing job, require
   token-authenticated JSON run/jobs requests, preserve provider identity and
   attempts, and map the status-only failure to recovery-required `Unknown`.
2. **Merge and validate the Temper feature.** Merge the API-only implementation
   after its focused backend tests, affected root live scenarios,
   `cargo dev-scenario-check`, and `./.temper/pre-pr` pass. Do not change the
   persistent Forgejo service during this phase.
3. **Migrate the persistent service with the old compatible Temper deployment
   still running.** An operator follows the official Forgejo upgrade guidance,
   rehearses the complete upgrade on a copy of production data, takes a fresh
   production backup, upgrades to exactly 16.0.1, and verifies repository,
   webhook, git, and runner behavior. The operator then calls `/api/v1/version`
   and the per-provider-run jobs endpoint with a deployment token and confirms
   the expected JSON identity/status fields. Keep the prior Temper deployment
   running until this proof is complete; it remains the rollback-compatible
   application side of this service-only change.
4. **Deploy the API-only Temper binary.** Only after phase 3 succeeds, stop the
   affected Temper engine/standalone service, remove the obsolete `ci_user` key
   from the deployed `[forge]` configuration, run the new binary's configuration
   check, and restart with the API-only build. Confirm exact-current-head CI
   observation and successful landing before declaring the rollout complete.

If any proof fails, stop. Restore or repair the Forgejo service according to the
operator's rehearsed provider rollback plan while retaining the prior compatible
Temper deployment. Do not deploy the API-only binary against an unproven or
older service, and do not remove `ci_user` early merely to make new configuration
validation pass.

## Backup and rehearsal evidence

Before the persistent change, record outside the repository:

- Forgejo's current reported version and the target 16.0.1 binary provenance;
- a restorable backup of the database, repositories, configuration, secrets,
  Actions state, and other provider-managed data required by Forgejo's official
  backup guidance;
- the rehearsal copy, commands, duration, migration output, and restore result;
- the persistent service stop/start window and rollback decision point;
- token-authenticated version, run discovery, and per-run jobs responses with
  credentials redacted;
- one real runner success and one real status-only failure on the persistent
  service, including provider run/job/attempt identity and exact commit SHA;
- webhook delivery, git clone/push, and current-head landing smoke results.

Store no production backup, token, cookie, password, raw service data, or
migration log in this repository.

## Configuration cutover

The new configuration contract contains the Forgejo URL, token identities,
repositories, webhook secret, and CI poll settings. It contains no CI scraping
user or password. Role-user passwords may still exist in provisioning systems
when needed to create users or mint tokens; they are not engine/worker CI inputs.

Because the new schema rejects `ci_user`, perform its removal in the same stopped
service window as the binary cutover, after the persistent API proof and before
the first API-only restart. For systemd deployments, update the reviewed
`config.toml`/credential source, run:

```sh
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check --component engine
# or: check --component standalone
```

with the **new** binary, then restart the selected topology. Never run standalone
and split services over the same state directory.

## Post-restart proof

After restart, require all of the following before ending the maintenance
window:

- `/api/v1/version` still reports 16.0.1;
- Temper's Forge requests authenticate with a token and CI reads use only run
  discovery plus `/actions/runs/{provider_run_id}/jobs`;
- the observed job IDs, run ID, attempt, task identity, and commit ownership are
  stable across list/get reads;
- a current-head success permits landing;
- a current-head status-only `failure` remains `Unknown`, recovery-required,
  leaves the PR open, and does not dispatch writable repair or advance the head;
- no request reaches `/user/login`, repository Actions HTML, a live-view POST,
  or `/actions/tasks`.

The ignored live backend contract and daemon scenarios are the repository-owned
proof of these invariants. Persistent migration and backup evidence remain
operator-owned and must not be simulated by editing fixtures or repository
state.
