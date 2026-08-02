# Production-shaped Rust change

This benchmark keeps task scale fixed while varying lineage context:

| Profile | Purpose |
| --- | --- |
| `compact` | Primary issue only; lower bound for context size. |
| `lineage-curated` | Full artifact hierarchy with unique decisions, evidence, risks, and non-goals retained once. |
| `lineage-heavy` | Near-verbatim primary, feature, plan, design, and research artifacts; production-shaped upper bound. |

Run all profiles with the same agent binary, provider/model, repetition count,
and cache conditions. Compare success and validation first, then turns, mutation
turns, input tokens, model duration, and wall time. The curated profile is useful
only if it retains compact/heavy correctness while approaching compact cost.

The curated projection deliberately removes repeated acceptance prose. It keeps
artifact identities and states so lineage remains inspectable, then preserves
only information that changes implementation: observable compatibility,
architecture ownership, normalization order, risks, evidence, and non-goals.
