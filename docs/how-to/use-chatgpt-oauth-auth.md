# Configure Smith LLM responders from Temper

Temper does not read provider auth files, hold model-provider API keys, or link
`pi_agent_rust`. For provider login, model ids, auth-file overrides, and live
provider preflight, read Smith's docs in `~/src/rust/smith/docs/`.

Temper only launches responder processes and treats their args/env as opaque
child-process configuration.

## Workflow role workers

Workflow role workers now receive concrete jobs from Temper and return structured
results: a branch/diff, a declared verdict with authored content, or a structured
failure. Configure provider auth in the external worker/agent process that runs
that job; do not pass Forge tokens or generic Forge mutation handles to the
agent.

For Smith-backed deployments, build and configure the Smith role-job worker from
the sibling Smith checkout, then pass only the provider variables Smith documents
(for example an auth-file path or model id) through that worker's configuration.
Temper still owns workflow validation and all Forge mutations after the worker
returns its result.

## Interactive profiles

Interactive LLM responders are selected through the generic interaction
deployment binding file, not through profile-specific Temper binaries or env
names. Dogfood's example product-manager profile generates that binding for you:

```sh
cd ~/src/rust/smith
cargo build -p smith-temper-agent-cli --bin smith-product-manager-responder

cd ~/src/rust/smith/examples/dogfood
./run.sh product-chat
```

For a custom profile, set the binding file's `responders.<id>.command`, `args`,
`cwd`, `env_allowlist`, and `timeout_secs`, then launch `temper-interaction repl`
or `temper-interaction serve` with `--spec` and `--bindings`. External clients
should call Temper's generic interaction service/API; Smith's process is only the
responder implementation.
