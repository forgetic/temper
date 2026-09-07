# Shared codebase-memory lifetime

The native 0.10.8 daemon serves a build/cache cohort across sessions under one OS
account. A Temper job owns its MCP frontends and their cancellation. The shared
daemon needs a separate owner: placing it below the first job's process boundary
allows recursive cancellation of that job to remove a daemon another job uses.
Keeping two frontend connections open does not change that ancestry.

On Linux, the first-party worker or standalone composition starts a transient
helper inside ordinary process containment, outside individual job containment.
The helper starts the operator-configured public MCP frontend with the session
`analysis` tool profile. This profile suppresses automatic background indexing,
watching and UI admission. It does not change daemon-global settings. The normal
serving frontend retains the operator's configured arguments and profile.

The helper is a child subreaper. After bootstrap closes, it remains alive while
its adopted native daemon/index descendants remain alive. Its outer containment
therefore retains recursive ownership without changing the default rule that
ordinary job/MCP owners clean all descendants when their payload exits. A shared
manager retains and joins completed owner monitors. Neither dropping one job's
admission handle nor losing that job's startup contract kills the shared owner.

Discovery stays connected until a fresh serving client has completed initialize,
tool discovery and the supported-provider contract. A closed lifecycle event
then releases bootstrap. A poisoned discovery connection is never reused. The
parent enforces a finite missing-admission deadline: expiry fences late release,
cancels the job and waits for ordinary recursive-empty cleanup before closing
bootstrap. A delayed job cannot outlive its bootstrap and cold-start another
daemon inside its job boundary.

The helper closes its own temporary session; it never issues `daemon start` or
`daemon stop`. Upstream decides when the final account session ends. Another
native client, another worker, or an indexing subscription can keep that cohort
alive. No permanent Temper provider daemon or system service is installed.
Worker/service shutdown remains an orderly operation across affected account
sessions. This design does not promise survival of the original worker's process
or systemd control group, and does not weaken `KillMode=control-group`.
Unexpected native daemon crashes during admission are a separate failure mode
from supported job cancellation; there is no claim of transparent recovery.

Manual first-party agent invocation without the parent lifecycle boundary has
no shared launch authority. Optional graph configuration falls back to filesystem
tools and required graph configuration fails startup. Low-level embeddings must
supply an explicit parent-owned admission. Disabled roles and third-party agent
commands do not create bootstrap helpers.

## Checkout source boundaries

Stable logical repository identity is preserved across prepared checkouts.
Provider mutation locks serialize index updates; they do not isolate a read from
another session rebinding the same logical project. The 0.10.8 session allowed
root restricts indexing admission, but its snippet read path resolves the shared
project's current root.

The installed build can report a successful incremental index while retaining
the previous checkout's root metadata, and an open session can retain an older
database generation. Temper still requires targeted confirmation of the active
root; an unconfirmed rebind is unavailable. A successful index response alone is
not sufficient admission evidence. The installed concurrency test forces fresh
native generations through supported indexing options and checks their roots
before testing source presentation.

Temper verifies source responses before model presentation or lineage recording.
Every snippet and nested code-search source/context fragment must exactly match
bounded bytes in the requesting checkout at its stated file/range. Relative paths
resolve under that checkout; foreign absolute paths, escapes, malformed metadata
and unverifiable multipart responses are unavailable. Range-less responses must
match a complete bounded local file. Optional stored signatures, docstrings and
return types are omitted because their read contract lacks an exact source range.
A rejected response contributes no successful source lineage.
It closes the run's existing not-ready circuit; recovery requires a fresh
admitted toolset rather than retrying the rejected serving client.

These checks detect ordinary rebinds and source A→B→A races without pretending a
Temper-local lock protects external provider clients. Identical bytes at the same
local location are safe to return. The check is a bounded local observation, not
a durable filesystem snapshot. Graph relationships, symbol metadata and coverage
still lack a provider-side atomic expected-root/monotonic-epoch fence in 0.10.8.
Status brackets and local fingerprints cannot prove graph or coverage snapshot
isolation during an external A→B→A rebind; do not use them for stronger exhaustive
claims. Read cited local source and qualify coverage accordingly.
