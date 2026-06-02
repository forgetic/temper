# Lesson 0020: Keep dogfood identities and PR diffs honest

## Tags

`dogfood`, `agents`, `forgejo`, `product-chat`

## Trigger

Forgejo issue #4 from the dogfood product-chat flow showed human turns authored
by `bot`; the filed issue requested work for `bot`; and the engineer merged PR #6
with only Temper prep/CI marker files instead of the requested product-chat fix.

## What went wrong

The dogfood product-chat wrapper reused the workflow `human` alias, which was
configured as `bot`, for actual user transcript turns. The live Forgejo engineer
path also reused an e2e/demo prep hook that creates a differing branch and a
synthetic CI-pass commit, but no real coding worktree changed product code.

## Steering for future agents

Do not treat live dogfood as a synthetic e2e. Product-chat human identity must be
configured separately from workflow role identities and must fail closed when the
real user's token is missing. Live engineer PRs must contain meaningful project
changes; if only Temper bookkeeping or no diff exists, stop/escalate instead of
opening or merging the PR. Store Temper operational state in metadata or
provider-specific refs, not committed files.

## Where this is now documented

`plans/dogfood-product-feedback/README.md` tracks the hardening plan.
