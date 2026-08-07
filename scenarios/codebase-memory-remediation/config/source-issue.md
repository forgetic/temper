Validate the codebase-memory agent integration on the live manifest runner.

Alias retries must remain on the original ordered worker. `retry_worker_topic` currently uses the alias only for attempt zero, so a retry can move to the raw topic's worker.

Use the graph evidence to locate the implementation and focused test, then read the selected `src/lib.rs` through the safe codebase-memory source tool before verifying exact contents with ordinary repository tools. The provider may report its stable project fresh, but the agent must rely on the explicit active-checkout rebind rather than a retired checkout. If codebase-memory becomes unavailable, do not immediately retry it: continue with conventional discovery. Repair `src/lib.rs` so every retry uses `canonical_topic` when it is present, preserve non-aliased behavior, and validate with `cargo fmt --check && cargo test --quiet`.
