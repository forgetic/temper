# Development workflow

How work reaches main. Work happens on local branches; main moves only by
merging a branch that passed every check below.

## 1. Before merging to main

On the branch's tip, rebased on main, so that what is checked is what main
becomes:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo nextest run --workspace --profile fuzzy
```

Then, once all four pass:

```sh
git checkout main
git merge --ff-only <branch>
```

## 2. The two test suites

- **The default suite** is every crate's unit tests and `tests/integration`:
  focused tests of expected behaviour (scenarios, referee tests, replay,
  facts changing nothing, memory against the worst case), and a cheap
  random world as a smoke test where one helps. It is what
  `cargo nextest run --workspace` runs, and it takes at most **10 seconds**.
- **The fuzzy suite** is `tests/fuzzy`: randomized tests, such as sweeps of
  random worlds and models driven at random, over many seeds. It is not
  run by default, only with `--profile fuzzy`, as the gate before merging
  to main, and it takes at most **1 minute**.

The budgets are enforced, not advisory: `.config/nextest.toml` gives each
profile a `global-timeout`, so a suite that runs past its budget fails, and
a per-test `slow-timeout` that ends a runaway test and fails it by name. A
change that breaks a budget is fixed by making tests cheaper, or by moving
randomized ones to `tests/fuzzy`. Raising a budget is a decision to take
explicitly, not a fix.

The budgets are wall time on the development machine (4 cores, 8 threads),
with nothing else building. Run the checks on an idle machine.

## 3. When a fuzzy seed fails

A failure names its seed, and a seed replays to the same run. Fix the bug,
then keep its seed: as a scenario in `tests/integration` if it shows
behaviour worth naming, or among the sweep's pinned seeds. A finding that
cannot be fixed yet joins its world's `FINDINGS`, which an ignored test
replays until it is fixed.

## 4. Status

As of 2026-10-03 the split of the randomized tests out of
`tests/integration` into `tests/fuzzy`, and `.config/nextest.toml` with its
budgets, are in progress on branch `faster-tests`. Until they land there is
no `fuzzy` profile, and the default suite runs the sweeps too (about 30
seconds).
