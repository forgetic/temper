# Agent guidance for temper

- Before merging anything to main, run the checks in
  `docs/development/workflow.md`; main moves only when they all pass. A
  change to Markdown files only skips them (same document).
- The default test suite (unit tests and `tests/integration`) takes at most
  15 seconds; the fuzzy suite (`tests/fuzzy`, randomized tests) at most 1
  minute. Keep new tests within these budgets: see the same document.
- temper is built on skein, the io and generic-protocol kit, and follows
  the foundation documents in the `docs/foundation` directory of skein's
  repository. temper's code and documents cite them by file name:
  - `programming-model.md`: how code is written; every crate follows it;
  - `testing-strategy.md`: how that code is tested;
  - `notes.md`: the questions still open, and why the memory strategy is
    what it is.
- temper's own design documents are in `docs/design/`; `testing.md` is how
  the testing strategy applies to temper.
