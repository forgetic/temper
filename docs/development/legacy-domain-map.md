# Legacy engine world map

This accounts for the two tables in `docs/plans/next-domain/07-cutover.md`, section 2, before the protocol cutover. The named tests use the new domain worlds. Component stories are joined through the root where a root route exists; the system worlds join their wire and agent paths in cutover step 07b.

## Stories

| Legacy story | New story or disposition |
|---|---|
| A web session says hello and finishes | `tests/engine/domain/tests/walking.rs`: `chat_claim_two_charged_turns_and_result_survive_a_lost_commit_completion`. |
| A chat parks and resumes from a snapshot | `tests/engine/domain/tests/root_routes.rs`: `a_chat_parks_and_resumes_from_its_transcript` and `a_chat_past_the_resume_limit_starts_fresh_with_the_tail_in_its_brief`. The new durable source is the transcript. |
| An issue is handed in; its change fails CI, is repaired, reviewed and landed | `tests/engine/domain/tests/forge_routes.rs`: `a_small_fix_made_in_a_chat_lands` and `a_change_failing_ci_is_repaired_reviewed_at_its_head_and_lands`. |
| A note is written by a run, corrected by a person and recalled | Deferred to the notes store work in later step 08, as the cutover plan's table says. The current brief and notes domains do not claim this story. |
| A plan is proposed, accepted, decided, grown within its envelope or beyond and landed | `tests/engine/domain/tests/root_routes.rs`: `a_members_goal_past_their_allotment_becomes_a_proposal_a_maintainer_accepts`, `a_proposal_reaches_its_covering_task_and_is_accepted`, `a_plan_of_spikes_a_choice_and_changes_runs_in_dependency_order`, and `a_batch_beyond_authority_is_refused_whole`; `tests/engine/forge/tests/simulation.rs`: `a_goals_issue_is_revised_and_closed_as_the_goal_finishes` and `a_change_produced_opened_checked_queued_and_landed`. The full cross-component path is owed to the new system world in step 07b. |
| A proposal is rejected | `tests/engine/domain/tests/root_routes.rs`: `a_proposal_rejected_tells_the_proposer_why`; the person sees the committed reason. |
| A change's CI never reports and it is held | `tests/engine/domain/tests/forge_routes.rs`: `a_change_whose_ci_never_reports_is_stalled_and_held`. |
| A person removes a tracking label | Goes: no label marks tracked work; the store owns task and issue identity. |
| A person garbles a record in a comment | Goes: domain records live in the ordered store, not forge comments. |
| A person watches and stops a run | `tests/engine/domain/tests/root_routes.rs`: `a_watched_run_shows_each_turn_as_it_commits` and `a_person_stops_a_run_and_releases_it`. |
| A supervisor wakes once for a burst | `tests/engine/tasks/tests/inbox.rs`: `a_coordinator_is_woken_once_by_a_burst`; the task is the coordinator. |
| Approval of an earlier head lands nothing | `tests/engine/forge/tests/simulation.rs`: `a_merge_decided_for_an_old_head_cannot_land_the_new_head`, `an_exact_head_approval_is_asked_again_after_a_clean_update`, and `a_clean_update_keeps_its_review_and_lands_the_new_head`. |

The old world's replay, facts and restart-cut cases map to `tests/engine/domain/tests/walking.rs`, `forge_routes.rs`, `roles.rs`, `escalation.rs`, and their `fuzzy_*.rs` sweeps. `effect_survives_each_commit_and_outbox_cut_without_a_second_issue` replaces the keyed comment/issue recovery path with a store-and-outbox one. The new tasks world separately cuts make, claim, terminal and cancellation decisions in `tests/engine/tasks/tests/restart.rs`.

## Referee rules

| Legacy rule | New observer and negative case |
|---|---|
| Protected landing requires green CI on the exact head and a person's approval | `tests/engine/forge/tests/simulation.rs`: `a_failed_ci_head_is_repaired_then_checked_again`, `a_gate_added_while_queued_takes_the_change_out_until_approved`, and `an_exact_head_approval_is_asked_again_after_a_clean_update`. The configured landing rules in `authority.md`, section 10, decide when a person is required. |
| Writes stay in deployment repositories | `tests/engine/forge/tests/simulation.rs`: `a_context_repository_cannot_receive_a_write` and `a_repository_with_another_deployments_branch_prefix_is_refused`; `tests/engine/domain/tests/referee.rs`: `authority_referee_rejects_a_person_without_a_grant`. |
| Keyed creations and outcomes happen once across restart | `tests/engine/domain/tests/root_routes.rs`: `a_call_asked_twice_across_a_restart_is_decided_once`; `tests/engine/domain/tests/forge_routes.rs`: `effect_survives_each_commit_and_outbox_cut_without_a_second_issue`; `tests/engine/domain/tests/referee.rs`: `once_referee_rejects_a_second_result_from_the_same_durable_task`. |
| Attempts increase; one live run per task; dependencies end first | `tests/engine/tasks/tests/referee.rs`: `nonmonotonic_attempt`, `terminal_of_wrong_attempt`, `assignment_before_dependency_done`; `tests/engine/domain/tests/referee.rs`: `order_referee_rejects_an_early_dependency_and_overlapping_runs`. |
| No plan work before a person accepts it | `tests/engine/domain/tests/root_routes.rs`: `a_members_goal_past_their_allotment_becomes_a_proposal_a_maintainer_accepts`, `a_batch_beyond_authority_is_refused_whole`; new work beyond authority requires an accepted proposal. |
| A call covered by grants is not refused as ungranted | `tests/engine/domain/tests/root_routes.rs`: `a_delegate_batch_and_its_named_answer_commit_together`, `a_batch_beyond_authority_is_refused_whole`; `tests/engine/domain/tests/forge_routes.rs`: `an_authorized_forge_read_returns_a_bounded_typed_answer`. Grant tests live with the authority domain; the root decides each call once. |
| A person's message reaches its run or the task ends | `tests/engine/domain/tests/root_routes.rs`: `words_typed_while_a_run_works_reach_it_once_committed`, `a_read_fence_takes_only_what_the_run_read`, `words_to_a_parked_chat_wake_it`; `tests/engine/tasks/tests/inbox.rs`: `a_read_fence_takes_only_offered_words`. |
| Every story ends within a bound | Each focused root and child world settles under its own step bound; `tests/engine/domain/tests/referee.rs`: `no_loss_referee_rejects_an_unfinished_cancelled_task` and `bounded_referee_rejects_more_live_tasks_than_the_world_allows`. The global focused and fuzzy suite limits are enforced by `.config/nextest.toml`. |
