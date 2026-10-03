# Agent guidance for temper

- Before merging anything to main, run the checks in
  `docs/development/workflow.md`; main moves only when they all pass. A
  change to Markdown files only skips them (same document).
- The default test suite (unit tests and `tests/integration`) takes at most
  15 seconds; the fuzzy suite (`tests/fuzzy`, randomized tests) at most 1
  minute. Keep new tests within these budgets: see the same document.
- Every crate follows skein's programming model,
  `~/src/rust/skein/docs/design/programming-model.md`, which temper's code
  and documents cite as `programming-model.md`. temper's own design
  documents are in `docs/design/`; `testing.md` is how temper is
  tested.
