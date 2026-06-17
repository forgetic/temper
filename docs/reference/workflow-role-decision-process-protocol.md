# Workflow role job result contract

Workflow role agents no longer use a standalone selector contract.
Temper assigns one concrete role/action job and treats the worker/agent result as
the source of truth for what happened.

Use these current references instead:

- [Worker/Daemon wire protocol](worker-daemon-wire-protocol.md) for job
  assignment, heartbeats, structured results, failures, and lease release.
- [Workflow runtime execution](workflow-runtime.md) for how Temper validates and
  applies workflow transitions and Forge mutations.
- [ADR 0022: Workspace executor and verdict routing](../adr/0022-workspace-executor-and-verdict-routing.md)
  for the workspace/verdict design.

## Current role-job shape

1. Temper scans workflow queues and selects an eligible item.
2. Temper assigns a worker a concrete job containing the role, repository,
   queue, artifact context, workflow action, checkout capability, and allowed
   verdict vocabulary.
3. The worker prepares the requested workspace and starts the configured role
   agent.
4. The agent completes the assigned job and returns one structured result:
   - a branch/head diff and summary for writable implementation work;
   - a declared workflow verdict with authored body, review text, or child issue
     content when the assigned action declares those outputs;
   - or a structured failure/rejection with a clear class and message when the
     job cannot be completed.
5. Temper validates the result against the compiled workflow and applies any
   labels, comments, PR creation, reviews, merges, or other Forge mutations
   itself.

Agents may inspect workspaces, edit checked-out files when granted a writable
checkout, push the branch identity supplied by the worker, and author result
content. They do not receive Forge tokens or generic Forge mutation handles.

If a required workspace or provider binding is unavailable, deployment validation
should avoid assigning the job where possible; otherwise the worker should report
a structured failure so operators can see and fix the configuration.
