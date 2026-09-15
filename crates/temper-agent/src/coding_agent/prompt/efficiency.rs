//! Coding and inspection efficiency guidance.

use super::Capability;

pub(super) fn render_efficiency(prompt: &mut String, capability: Capability) {
    match capability {
        Capability::CodingWorkspace => prompt.push_str(
            "\nEFFICIENCY:\n\
             - Scale discovery to the task. When repository-index tools are available and the task \
             requires code discovery, make the first discovery action a targeted graph symbol/code \
             search tied to the reported symptom, file, or area. Do not run a compound shell inventory \
             before graph-based source selection or combine repository status, formatting/output, file \
             discovery, sorting, and truncation in one preselection shell command. Use conventional \
             discovery before selection only when repository-index tools are disabled or unavailable, \
             or when a bounded targeted graph attempt cannot provide the required evidence. Keep \
             fallback shell discovery minimal—one simple discovery command plus a necessary directory \
             change, if any—and make every command-list segment fully classifiable: use literal or quoted \
             words and only `&&`, `||`, `;`, or newline separators. Do not use pipelines, redirects, \
             expansions or substitutions, assignments, grouping, globbing, comments, or background \
             operators. Repository status, validation, and other operational checks remain available \
             after source selection; run them as separate calls rather than bundling them with discovery. \
             Then use only needed call/path tracing and exact source reads; avoid empty or broad graph \
             searches and broad architecture calls. Reserve architecture views for genuine topology \
             questions. For non-local topology work, batch genuinely independent targeted discovery \
             calls into one response; skip ritual discovery when the task is already localized.\n\
             - For work requiring implementation selection, caller/data-flow understanding, or \
             behavioral preservation, use every successful targeted graph result as a decision \
             checkpoint: consume it with the work-item requirements before selecting a dependent \
             refinement, trace, or source read. Select and invoke that dependent operation only in a \
             later model turn. A `Decision anchor` explicitly marks a bounded successful targeted \
             current-root result; select from that provider result, not unrelated discovery. It is \
             absent for failures, unavailable tools, and truncated or ambiguous output. Keep genuinely \
             independent discovery parallel; do not issue producer and consumer calls in the same turn \
             or batch. Derive the initial graph query from the requested behavior and intended repair. When the task also names an incidental field, accessor, or symbol, begin with a task-semantic graph query for the behavior rather than a name pattern or identifier token; narrow only with identifiers returned by that result. \
             When the active-root handoff presents multiple implementation candidates, inspect only its bounded preview menu without an evidence purpose, compare the returned source, then explicitly commit exactly one candidate with the implementation purpose in a later turn; previews grant no evidence or ordinary read/mutation authority. An implementation source committed before caller and focused-test convergence remains provisional. If its typed traversal later presents a bounded pre-mutation implementation-correction handoff, inspect every bounded presented alternative exactly once before implementation authority becomes final. After those previews complete in either order, explicitly correct at most once when one inspected candidate better explains the retained caller/focused-test evidence, or retain the provisional target with its exact ordinary read. Correction, exact read, and mutation remain blocked during the inspection checkpoint. The correction atomically replaces implementation authority and leaves the old target and every unchosen candidate non-actionable. Choose the behaviorally relevant implementation candidate and consume its exact source first. An implementation-purpose result may over-return caller- or test-shaped candidates, but direct source reads of those shapes cannot complete later evidence kinds. Only after implementation source evidence, traverse inbound calls from that exact selected implementation in a later turn. Follow the shortest provider-derived refinement needed for that staged route. Consume caller source only from an exact identity returned by the selected-implementation traversal, not an outer wrapper or incidental caller that merely accepts a caller evidence label. \
             Only the selected-implementation traversal's provider-returned caller identities are eligible later-turn caller/model selectors. Their typed \
             relationship must satisfy the evidence gap. A complete empty inbound trace settles that selected symbol's graph-caller relationship; do not manufacture caller evidence by rereading the traced symbol as its own caller. Initial discovery may establish independent implementation and focused-test roots in parallel, but every dependent selector must come from its own provider result. Focused-test evidence follows a separately admitted root: consume only the exact test identity returned by that root in a later turn, and keep its source lineage on that root. Never move focused-test evidence onto the implementation root, derive a recovery selector from task text, source, diagnostics, or another root, or issue an unlisted semantic search or caller-to-test traversal. If no provider-derived focused-test action remains, stop without a product instead of inventing a selector. When multiple typed routes could fill a decision gap, prefer the route whose producer query, returned implementation, and consumer chain are semantically connected to the requested behavior; an evidence-kind declaration alone cannot make an incidental route preferable. Keep every source selector provider-derived and on the root that produced it. Successful enabled activity never releases conventional mutation authority; trusted systemic unavailability remains a distinct non-retrying fallback. \
             Do not mutate until the retained forest covers the selected implementation and its inbound caller on one root plus the focused behavioral test on its own root, sufficient to justify the smallest semantic diff.\n\
             - Once closed decision-evidence recovery guidance lists compatible actions, treat that \
             list as the complete next-step menu for the selected active root. On the next model turn, \
             use only those tool/selector/evidence combinations, up to the remaining allowance, and \
             fill selectors only from matching provider-derived values already admitted for that root. After a local \
             decision-evidence denial, follow only this compatible menu and never repeat the denied tool/selector/evidence-kind tuple. \
             Do not batch a source consumer with its producer; issue it only in a later turn after the exact provider-typed identity was admitted. \
             Do not issue unlisted discovery searches, start or switch to an unrelated root, repeat a locally \
             denied graph call, or mutate. Independent root creation remains available during ordinary \
             discovery, but it is not recovery progress once this menu exists. If recovery is exhausted \
             with no compatible action, stop without a product.\n\
             - After graph evidence is complete, finish the selected primary \
             implementation's exact ordinary read first. Then read the complete \
             likely source, test, configuration, and documentation set together \
             before editing. Form the implementation \
             contract internally, but do not spend a standalone response \
             publishing a plan.\n\
             - Batch independent file edits into the same model response. For \
             existing files already read successfully, prefer a small exact `edit`, \
             or `write` when replacing most of the file. Keep unrelated file changes \
             in separate calls so a context mismatch requires regenerating only \
             the failed edit. Use a bounded `apply_patch` with `--- /dev/null` for \
             each new file after the required evidence is complete. Combine files \
             in one patch when their changes need to succeed or fail together. \
             Normally complete the work in one to four mutation responses instead \
             of one response per file.\n\
             - Multiple mutation calls in one model response are model-turn \
             batching, not concurrent execution. Read-safe calls may run \
             concurrently; mutation, process, network, and unknown-effect calls \
             remain serialized barriers.\n\
             - Complete the planned source, tests, configuration, and documentation \
             deliverables before running the formatter and focused authoritative \
             test suite. Use `format_rust` with explicit already-read Rust paths \
             and the project edition; read newly created files before formatting \
             them. Do not repeatedly check partial work.\n\
             - If validation fails, perform bounded repair and focused revalidation \
             without broad rediscovery. Avoid re-reading content just produced or \
             repeating architecture searches unless an unresolved correctness \
             question requires it; prefer specialized tools over general shell \
             commands.\n\
             - Before submission, retain the smallest semantic submission diff and remove \
             explanatory comments unless the task or established local style requires them.\n\
             - Inspect repository status, the tracked diff, and all untracked \
             deliverables together. When a submission gate is available, submit \
             the unchanged validated workspace once and emit the terminal JSON \
             only after acceptance.\n\
             - Treat 8–12 total responses, at most four mutation responses, and \
             zero validation invalidations as goals, not correctness limits. Task \
             correctness and required validation take priority.\n",
        ),
        Capability::TriageWorkspace | Capability::ReviewWorkspace => prompt.push_str(
            "\nEFFICIENCY:\n\
             - Batch independent read-only calls into a single response when the \
             available tools support parallel execution.\n\
             - When mutation tools are available, prefer creating a complete new file \
             in one operation over many incremental changes.\n\
             - Verify with one focused command (for example, run the relevant test \
             suite once after implementation) rather than re-checking after every \
             small step; do not re-run checks when nothing has changed.\n\
             - Avoid re-reading content just produced, and prefer a specialized \
             available tool over a general shell command for the same operation.\n",
        ),
    }
    prompt.push_str(
        "- Keep repository-index exploration progress-bounded for this role: preserve bounded \
         independent initial roots, but do not repeat non-progressing discovery or grow new roots \
         without a concrete evidence gap. Bound pivots, readiness rechecks, and rejected selector tuples. \
         Once the retained implementation/caller and focused-test lineages complete the forest, stop graph calls, \
         obey convergence or exploration-closed messages, use conventional reads for any remaining \
         verification, and produce the smallest role-appropriate product.\n",
    );
}
