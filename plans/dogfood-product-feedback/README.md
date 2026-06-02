# Dogfood product-feedback loop hardening — plan

This plan fixes the live dogfood failure observed in Forgejo issue
[#4](https://git.ekanayaka.io/ai/harness/issues/4), the filed intake issue
[#5](https://git.ekanayaka.io/ai/harness/issues/5), and the merged but empty PR
[#6](https://git.ekanayaka.io/ai/harness/pulls/6).

## Findings

- Product-chat human turns were authored by `bot`, not `free`. The dogfood
  wrapper currently reuses the workflow `human` alias, and
  `examples/dogfood/config/dogfood.env` maps that alias to `bot`.
- The filed issue body consequently said `requested-by: bot`, even though the
  real requester was `free`.
- The engineer opened and merged a PR containing Harness bookkeeping only:
  `.harness-pr-prep/*`, `.harness-ci/*`, and dogfood CI setup history. It did not
  change product-chat behavior.
- Root cause: the current real engineer is a workflow-decision adapter. Its
  Forgejo prep hook only creates a differing PR head plus a synthetic CI pass
  marker; it does not run a coding tool against a checkout.

## Direction

Dogfood must be honest before it is automatic: comments should preserve the
actual human identity, generated PRs should contain product code changes only,
and Harness operational state should not appear as repository content. If a
coding agent cannot produce a meaningful diff, the workflow should stop with a
visible comment/escalation instead of opening or merging an empty PR.

Keep provider-specific git/ref operations out of `harness-forge`; put live
Forgejo dogfood mechanics in `harness-production` / `examples/dogfood`, and keep
hermetic test/demo shortcuts isolated from the live dogfood repo.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Forensic guardrail and cleanup.**
   - Add a temporary dogfood safety switch so the engineer cannot auto-open or
     auto-merge PRs unless a real coding implementation path is enabled.
   - Add reviewer/owner guardrails that reject PRs whose diff is only Harness
     bookkeeping paths or whose diff is empty after excluding ignored internal
     paths.
   - Remove existing `.harness-pr-prep/` and `.harness-ci/` files from the live
     repo with a normal human-reviewed cleanup PR.
   - Make dogfood CI provisioning explicit/idempotent: do not silently commit CI
     workflow updates during ordinary product-chat or worker startup.

2. ☑ **Correct product-chat identity and command handling.**
   - Split product-chat human identity from the workflow `human` role. Add a
     dogfood config key such as `DOGFOOD_PRODUCT_CHAT_HUMAN_USER=free` and parse
     a matching token separately from `HARNESS_FORGEJO_TOKEN_HUMAN`.
   - Fail `./run.sh product-chat` with a clear message when the requested human
     token is unavailable; do not silently fall back to `bot` for transcript
     authorship.
   - Ensure transcript issue creation, human comments, and `requested-by:` all
     use the configured human identity.
   - Centralize command handling for REPL and service API so `/help` returns the
     command list locally, does not get sent to the LLM as ordinary prose, and is
     covered by offline tests.

3. ☐ **Replace repository-content bookkeeping with transparent git refs.**
   - Stop creating `.harness-pr-prep/*` files just to make a PR head differ.
     A PR branch should differ only because the coding agent made product code
     changes.
   - Stop creating `.harness-ci/*` sentinel files in live dogfood. Live dogfood
     CI should judge the actual head commit with the real repo workflow (for
     now, `cargo dev-check`); synthetic marker CI remains only in throwaway
     reference demos/tests where it is documented as a shortcut.
   - If the Forgejo integration needs auxiliary state, store it as provider-
     specific refs such as `refs/harness/<purpose>/<key>` or hidden Forgejo
     metadata/comments, never as files in the PR diff.

4. ☐ **Add a real coding-plane seam for the engineer.**
   - Introduce an `EngineerCoder`/`CodingWorkspace` seam that checks out the PR
     base, gives the coding agent the issue context and transcript backlink, runs
     the agent in a sandboxed worktree, and returns a candidate diff.
   - The LLM role agent still decides through `RoleTools`; only the coding seam
     can edit files and create commits, and it must not receive Forgejo tokens or
     broad workflow mutation tools.
   - Commit the produced code diff to `agent/pr-for-code-N` under the engineer
     identity, then open the PR. If no meaningful diff is produced, comment on
     the issue and leave it queued/escalated instead of opening a PR.
   - Add fake-coder tests for success, no-diff, failing validation, and retry
     idempotency; gate live coder validation behind env flags.

5. ☐ **End-to-end dogfood validation.**
   - Re-run the product-chat flow for the `/help` bug and verify the transcript
     has `free` human comments and `product-manager` replies.
   - File the draft and verify the intake issue says `requested-by: free`.
   - Let the engineer work the issue and verify the PR diff contains the actual
     product-chat fix, no `.harness-*` files, no synthetic CI marker commit, and
     no hidden Harness bookkeeping commits.
   - Run `cargo fmt --all`, `cargo dev-clippy`, `cargo dev-check`, plus focused
     `harness-production` product-chat and dogfood-script tests.

## Acceptance criteria

- A product-chat transcript preserves human/product-manager identities in
  Forgejo.
- `/help` behaves consistently through the REPL and service API.
- Dogfood workers cannot merge a bookkeeping-only or no-op implementation PR.
- Live dogfood PRs contain only project changes and normal CI results.
- Any unavoidable Harness runtime state is stored in metadata or provider-specific
  refs, not as files committed to the repository.
