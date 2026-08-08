Repair delivery-worker retry selection with the smallest semantic change.

First use the successful provider-shaped decision chain for this work item:

1. targeted graph discovery for `canonical delivery worker selection`;
2. refine the returned `delivery_worker_topic` target;
3. trace that target to its caller/model context;
4. read the selected `delivery_worker_topic` implementation from the confirmed
   current checkout root; and
5. use the successful implementation-source result to read its declared
   focused behavioral test from that same root.

Do not batch a producer with its consumer, use broad architecture or inventory
calls, or make additional codebase-memory calls. Only after the source evidence
is complete, update `src/lib.rs` so `canonical_topic` is selected whenever it
is present, preserve the unaliased behavior, and make no explanatory or
unrelated changes. Validate with `cargo fmt --check && cargo test --quiet`.
