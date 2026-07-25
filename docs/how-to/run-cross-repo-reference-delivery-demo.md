# Run the cross-repo reference-delivery demo

This recipe runs the operator-facing cross-repo fan-out path in
`examples/reference-delivery/`. It starts with one parent intake issue in
`acme/service` and deterministic workers fan it out into child code issues in
both `acme/service` and `acme/service-canary`.

The cross-repo fan-out path is the checked-in default for `./run.sh start`; the
reviewer-gated one-repo demo remains available as `./run.sh single-repo`.

## Prerequisites

Follow `examples/reference-delivery/README.md` for the shared Forgejo and runner
requirements. The multi-repo path uses deterministic in-tree fake workers rather
than jig, so it additionally builds `temper-testing-worker` from this workspace
and does not require provider credentials.

In short:

- let the launcher resolve the Bench-pinned Forgejo `16.0.1` and
  `forgejo-runner` `12.12.0` binaries, downloading and checksum-verifying
  missing Linux-amd64 assets in `.cache/forgejo/`;
- run on a host that permits host-mode Forgejo Actions jobs;
- let the launcher build the root `temper` binary and the `temper-testing-worker`
  binary.

## Run

```sh
cd examples/reference-delivery
./run.sh start        # ./run.sh multi-repo is an alias
```

The launcher boots a throwaway Forgejo, verifies that `/api/v1/version` reports
the Bench-pinned `16.0.1` release, registers a host-mode runner, provisions
exactly two repositories (`acme/service` and `acme/service-canary`), starts the
deterministic architect/engineer/reviewer/mechanical worker fleet across that
repo set, and only then files one unlabeled parent intake in `acme/service`.

The architect creates one ready code child per target repository. The engineer
opens real implementation PRs, the real Forgejo runner executes CI, the reviewer
approves, and the mechanical worker merges PRs once review + CI + dependency
gates are satisfied. The parent source issue unblocks and closes only after both
child issues have landed.

## Observe live state

While `./run.sh start` is still running, use the printed Forgejo URL and the
retained logs to inspect progress. The expected converged state is:

- both configured repositories are readable;
- the source repo has exactly one parent intake and the target repo has no
  duplicate parent intake with that title;
- the parent records exactly two child dependencies, one per configured repo;
- each child carries parent/correlation metadata and is closed; and
- the parent is closed no earlier than the latest child landing.

## Manual Forgejo checks

Open Forgejo before running `./run.sh stop` and confirm:

1. `acme/service` has the single parent intake issue;
2. `acme/service` and `acme/service-canary` each have one child code issue;
3. each child has a merged implementation PR in its own repository;
4. each implementation PR has an approving review and a successful Actions run;
5. `acme/service-canary` has no duplicate copy of the parent intake; and
6. the parent issue closes after the child issues close.

For event/log names, see `examples/reference-delivery/observability.md`.
