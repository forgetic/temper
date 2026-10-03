# Agent guidance for temper

- Before merging anything to main, run the checks in
  `docs/development/workflow.md`; main moves only when they all pass.
- The default test suite (unit tests and `tests/integration`) takes at most
  10 seconds; the fuzzy suite (`tests/fuzzy`, randomized tests) at most 1
  minute. Keep new tests within these budgets: see the same document.
- Design documents are in `docs/design/`; `programming-style.md` is the
  style every crate follows, and `testing-pyramid.md` how temper is tested.
