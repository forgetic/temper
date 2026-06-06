# `temper-provision-forgejo` tweaks: provision onto an existing, content-bearing repo

## Why

Smith wants to run the basic-delivery workflow against a **real, existing repo**
(`ai/smith`) on a **shared org** (`ai`, which also hosts `temper`) on the local
Forgejo instance, instead of a throwaway repo. Today `provision_world`
(`crates/temper-forgejo-provision/src/provision.rs`) is built for throwaway
repos and does two things that are wrong for a real target:

1. **It rewrites the repo's CI + history.** After ensuring the repo, it
   `commit_file(WORKFLOW_PATH, CI_WORKFLOW, …)` (a commit-message-marker CI
   workflow) and `commit_ci_sentinel(…)` onto the default branch. Against
   `ai/smith` this would clobber the project's own
   `.forgejo/workflows/ci.yml` and litter `main` with provisioning commits.
2. **It grants org ownership.** Every role user and the `bot` is added to the
   org **Owners** team (`add_team_member(owners_team, …)`). On the shared `ai`
   org that makes `architect`/`engineer`/`bot` **owners of every repo in the
   org**, including `temper` — far more than the dogfood needs.

We need a mode that provisions **identities + labels + webhook** onto a
pre-existing repo **without touching its contents**, and with **repo-scoped
access** rather than org ownership. All changes must default to today's behavior
so `reference-delivery` / `basic-delivery` throwaway flows are unaffected.

## Current behavior (reference)

`provision_world(base_url, admin_token, owner, name, roles, default_branch, workflow)`:

1. `forgejo_rest::ensure_org`
2. `forgejo_rest::owners_team_id`
3. per role: `create_user` → `add_team_member(owners_team)` → `mint_user_token`
4. `bot`: `create_user` → `add_team_member(owners_team)` → `mint_user_token`
5. `forgejo_rest::ensure_repo`
6. `upsert_labels`
7. `forgejo_rest::commit_file(WORKFLOW_PATH, CI_WORKFLOW, …)`  ← **marker CI commit**
8. `forgejo_rest::enable_actions`
9. `commit_ci_sentinel(…)`  ← **sentinel commit**

`provision_and_seed(...)` then calls `provision_world` and, when a webhook URL is
given, `forgejo_rest::ensure_repo_webhook`.

Helpers live in `crates/temper-forgejo-ops/src/forgejo_rest.rs`. Note these are
already **re-run safe**: `accept_or_conflict` treats `409/422 … exist/already/
member` as benign, and `mint_user_token` is documented not to fail on a
duplicate-token conflict. So creating users/labels/repo/webhook against an
existing instance is already idempotent; the only re-run hazards are items 7 and
9 (history-mutating commits) and the over-broad Owners membership.

## Proposed changes

### 1. `--existing-repo` flag (manage identities/labels/webhook only)

- Add `ProvisionArgs.existing_repo: bool` (default `false`) in
  `crates/temper-forgejo-provision/src/provision_args.rs`.
- Thread it into `provision_world` (new param or an options struct).
- When set:
  - `ensure_repo` must **require the repo already exists**: error clearly if it
    is absent rather than silently creating a bare repo (the operator named a
    real target by mistake). Implement as a `GET /repos/{owner}/{name}` check, or
    add a `require_repo` helper alongside `ensure_repo`.
  - **Skip step 7** (`commit_file` of the marker CI) and **skip step 9**
    (`commit_ci_sentinel`). The repo owns its own
    `.forgejo/workflows/ci.yml`; provisioning must never overwrite it.
  - **Keep step 8** (`enable_actions`) — idempotent, and we want Actions on.
  - Labels (step 6) and the webhook stay (both idempotent upserts).

Rationale for folding the CI skip into `--existing-repo` rather than a separate
`--skip-ci-commit`: there is no realistic caller that wants the marker CI pushed
onto a repo it declares pre-existing. Keep the surface minimal. (If a future
caller ever needs the opposite, add a focused `--seed-ci yes|no`; do not add it
speculatively now.)

### 2. `--access org-owners|repo-collaborator` flag

- Add `ProvisionArgs.access: AccessScope` with
  `enum AccessScope { OrgOwners, RepoCollaborator }`, default `OrgOwners`
  (today's behavior).
- `OrgOwners`: unchanged — `add_team_member(owners_team, login)` for each role
  user and the bot.
- `RepoCollaborator`: do **not** add anyone to the Owners team. Instead grant
  each identity a **repo-scoped collaborator permission** on `owner/name`:
  - role users (`architect`, `engineer`): `write`
  - `bot`: `write` (sufficient to merge PRs and to read Actions status over the
    web UI per ADR-0019; use `admin` only if a concrete need appears — document
    the choice in the code and the how-to).
  - With `RepoCollaborator` we still `ensure_org` (the org must exist) but never
    touch the Owners team.

### 3. New `forgejo_rest` helper

In `crates/temper-forgejo-ops/src/forgejo_rest.rs`:

```rust
pub async fn add_repo_collaborator(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    login: &str,
    permission: &str, // "read" | "write" | "admin"
) -> Result<()>
```

- `PUT /repos/{owner}/{name}/collaborators/{login}` with body
  `{ "permission": "<permission>" }`, routed through `accept_or_conflict` so a
  re-run that re-grants an existing collaborator is benign.
- (Optional) `require_repo(client, base, token, owner, name)` for the
  `--existing-repo` existence check.

## Arg parsing + usage (`provision_args.rs`)

- Parse `--existing-repo` (bool, no value) and `--access <org-owners|
  repo-collaborator>` (reuse a small parser like the existing `parse_bool`).
- Extend the `USAGE` constant to document both flags and that `--existing-repo`
  skips repo creation and CI provisioning (it only ensures labels, webhook, and
  Actions enablement on a repo that must already exist).
- Validation: `--existing-repo` against an absent repo → clear provision-time
  error. `--access` with an unknown value → parse error listing the two options.

## Tests

- `provision_args.rs` unit tests: parse `--existing-repo`; parse both `--access`
  values; default back-compat (no flags ⇒ `OrgOwners`, `existing_repo=false`);
  invalid `--access` errors; `--existing-repo` does not conflict with `--seed-*`.
- Behavior tests (against `temper-forgejo-fixture`, mirroring existing provision
  coverage):
  - `--existing-repo`: assert **no commit** to `WORKFLOW_PATH` and **no
    sentinel** commit, and that a pre-seeded file in the repo is untouched;
    assert labels + webhook + Actions still applied; assert a **missing** repo
    errors.
  - `--access repo-collaborator`: assert role users + bot are **not** Owners-team
    members and **are** repo collaborators with the expected permission; assert
    `bot` can still merge (or at least is granted `write`).
- Keep all existing throwaway-path tests green (defaults unchanged).

## Docs

- Document `--existing-repo` and `--access` in Temper's provisioning reference /
  how-to.
- Cross-link from Smith's deployment how-to (`docs/how-to/run-local-delivery.md`,
  forthcoming) which is the first caller.

## Backward compatibility

All new flags default to current behavior: throwaway provisioning still creates
the repo, commits the marker CI + sentinel, and joins the Owners team. No example
launcher changes are required by this work.

## Intended Smith caller

```sh
TEMPER_FORGEJO_ADMIN_TOKEN=<agent admin token> temper-provision-forgejo \
  --base-url http://127.0.0.1:3000 --owner ai --name smith \
  --existing-repo --access repo-collaborator \
  --workflow ~/.config/smith/workflow.json \
  --webhook-url http://127.0.0.1:<trigger-port>/forgejo/webhook \
  --webhook-secret-file ~/.config/smith/secrets/webhook-secret \
  --seed-intake no \
  --out ~/.config/smith/secrets/roles.env
```

This creates `architect`/`engineer`/`bot` with repo-scoped `write`, upserts the
six basic-delivery labels, registers the wake webhook, enables Actions, and
writes the role secrets file — **without** creating the repo, committing any CI,
or granting org ownership. Smith's own `.forgejo/workflows/ci.yml` (already in
the repo) remains the sole CI definition, and intake issues are filed separately
by the `agent` user.
