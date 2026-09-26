# Verify codebase-memory evidence and delegate findings

Use **Verify** by default: discover the task's relevant symbols and relationships,
read exact source, and check coverage for every source path you cite. **Scout**
reports narrow provisional positive findings. **Auditor** supports broader
conclusions only within an explicit checked scope, current generation, complete
relevant graph and coverage pagination, and stated limitations. These are
reporting contracts; neither a tier nor a clean result proves completeness.

When advertised with the supported schema, Temper exposes the read-only
`codebase_memory_check_index_coverage` diagnostic. It accepts prepared project
aliases (defaulting to the primary repository), up to 16 repository-relative
`paths`, and up to four `scopes`. Use `.` for a repository scope. Paths must be
canonical relative paths; absolute paths, traversal, symlink escapes and unknown
project aliases are rejected before provider dispatch.

```json
{"paths":["src/route.rs"],"scopes":["src"],"scope_limit":16}
```

Coverage is useful at verification time, including after discovery converges.
It never reopens the discovery budget, increases recovery attempts, creates
lineage/source authority, or replaces the parent's successful ordinary exact
read before changing an existing file.

Create files with an explicit unified `apply_patch` section whose old marker is
`--- /dev/null`. Temper verifies that the destination is absent inside the
workspace; new subdirectories are allowed, symlink ancestors are rejected.
Creation requires completed graph evidence and any pending correction inspection,
or an already released provider-unavailable fallback. A failed `read` never
authorizes creation. Each existing file in the same patch still needs its own
successful exact read. The tool repeats the absence check at execution and rejects
the whole patch if a creation destination has appeared or any target is invalid.

After discovery converges, use the ordinary `read` tool on the selected primary
implementation. Once that read succeeds, read each existing companion source,
test, or documentation file needed by the change. Those successful exact reads
authorize edits to their own targets under the same completed implementation
decision. Reads made before the primary read, failed reads, and unverified paths
do not authorize companion edits. A mixed patch is denied if any existing target
has not been admitted. Companion reads do not add graph evidence or release an
incomplete recovery; their authority depends on the primary read remaining
current.

The response distinguishes `clean`, `flagged`, `stale`, `unavailable`, and
`malformed`, retaining bounded generation/freshness metadata, exclusions, source
flags and scope pagination. Clean means no recorded gap. Read flagged, skipped,
excluded, changed or unavailable source directly or limit the claim. An absent
coverage capability is explicitly reported in both `auto` and `required` modes;
`required` retains its existing mandatory provider/index startup contract and
does not assert optional coverage availability. A timeout shares the provider's
bounded deadline and circuit breaker. Do not retry an unavailable provider.

Before absence, exhaustive inventory or dead-code claims, define the scope and
complete every relevant graph and coverage page. Scope entries enumerate recorded
**gaps**, not every source file. Follow each scope's `has_more`; use the returned
`next_scope_offset` and `generation` for the next page. The page limit is at most
32 and offset at most 4096. Temper links pages only within the same project,
checkout, source identity, generation and query bounds, retaining earlier flags.
It keeps at most 16 session receipts and 14 KiB of aggregate coverage. Narrow a
scope when a bound is reached and state what remains unchecked.

The provider's targeted status confirms the current checkout before and after
coverage. Because 0.10.8 status has no generation field, Temper compares two
bounded coverage responses and their generation metadata. It fingerprints HEAD
and the explicit files and scoped source, including untracked and excluded files.
This verification is bounded to 512 filesystem entries, 1 MiB per file and 16 MiB
total; missing cited paths/scopes, symbolic links and larger scopes produce
explicit unavailable freshness.
Git administrative files are excluded from that source fingerprint. Narrow the
scope or limit the claim when freshness cannot be checked. This is best-effort
observation at the tool boundary, not a lock against later external edits.

If subagents are already enabled, `investigate` and `delegate` accept optional
structured `graph_context`. Supply a session `evidence_id` from coverage,
`task_scope`, source paths with qualified symbols and source origins,
relationships/call chains, query bounds, pagination and explicit limitations.
The default tier is Verify; a scoped claim requires complete checked coverage and
complete declared graph pagination. Each cited child path must appear in the
receipt. Parent summaries are bounded; raw source snippets and provider
transcripts are not handoff fields. Temper adds normalized coverage diagnostics,
logical/actual project identity, generation and opaque checkout/source identities.

The wrapper revalidates the receipt before and after the child. Children retain
existing models and filesystem permissions, with no graph clients or inherited
lineage admission. Returned findings remain attributed evidence. The return
includes context and limitations, even when invalidated; changed source,
generation or current-root binding prevents successful reuse. Preserve these
limitations during compaction and recheck before relying on later findings.
Receipts are run-local: a fresh session must obtain new coverage rather than
silently reusing a previous session's identifier.

Run the mapped live contract with
`cargo dev-scenario-run scenarios/scoped-graph-evidence`. For real release
compatibility, use the installed-provider check in
[Upgrade and verify codebase-memory-mcp](upgrade-codebase-memory.md).
