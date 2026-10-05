# Isolated Forgejo v15 conformance

This opt-in check is outside default/fuzzy test budgets. It runs only
`/tmp/temper-forgejo-15/forgejo-15.0.0-linux-amd64`, creates a fresh temporary
work directory and SQLite database, binds HTTP and the webhook receiver to
loopback, and destroys the scratch instance after capture. It never invokes
the production `forgejo` wrapper or opens existing Forgejo state.

```sh
python3 tools/forgejo-conformance/run.py --output /tmp/temper-forgejo-observations
```

The groundwork questions in `docs/plans/next-domain/00-groundwork.md`,
increment 00a, add an isolated Actions runner:

```sh
python3 tools/forgejo-conformance/run.py --next-domain --output /tmp/temper-forgejo-next-domain
```

This also needs `/home/free/.local/bin/forgejo-runner` (observed with
v3.5.1). It registers only with the disposable instance, runs one job at a
time with the `temper-host:host` label, keeps registration and job files
under the scratch directory, disables its cache, and receives a minimal
environment. Its only workflow step prints a fixed marker and exits 1:
there are no downloaded actions, checkout steps or containers. Both
server and runner stop in `finally` blocks; scratch data is destroyed.
Allow roughly two minutes for startup and CI reporting; the updated-head
failure has a 75-second observation deadline. This is outside Cargo's
test suites and must run separately from idle-machine budget measurements.

The repository owner provisions the fixture. `reviewer` is a non-admin
write collaborator, and all 00a repository reads/effects use its token
except provisioning Actions and protection. Protection reads are checked
against an existing rule. `MAX_RESPONSE_ITEMS=2`, three files/commits and
four requested pages expose pagination rather than assuming it from a
single response. Merge probes include a moved head and a retargeted base.

The output identifies the binary version and SHA256, capture time and
isolation. HTTP request bodies and response documents are actual observations;
authorization headers, passwords and API token creation responses are excluded.
Webhook documents are captured after the receiver queues its 204. The synthetic
hook secret is fixed and used only for these disposable fixtures.
Registration tokens and runner logs are excluded too; only the presence
of the intentional stdout marker is recorded. A secret-value scan refuses
output if a fixture password or API token appears. A failed run leaves
`partial-observations.json` for inspection; only the completed
`observations.json` is suitable for importing fixtures.

The retained 00a capture is
`tests/forge/forgejo/fixtures/v15/next-domain-observations.json`. It selects
unmodified `00a-*` exchanges, the collaborator grant, and the update's
push/synchronized-pull-request webhooks from a completed run.
`next_domain` holds extracted observations, including the running
binary's Swagger Actions paths; it is separate from the wire exchanges.
Its conclusions and bounded fallbacks are in `domain/forge.md`, section
20: comparison ignores paging, write permission cannot read protection,
and the v15 supported REST API has no job-log route. Guessed `jobs` and
`logs` routes return 404 on that version. The migration target is now
Forgejo v16.0.5: its supported API lists a run's jobs and reads a job's
plaintext logs, with attempt selection and byte ranges. The client assumes
API log reads. Keep the v15 exchanges unchanged as historical evidence;
new v16 conformance captures must cover the supported routes specified in
`domain/forge.md`, 20.1.

Other protection settings and other Actions workflow/runner configurations
remain unverified. Tagged-source fixtures state their separate provenance.
This runner must not turn generated codec round-trips into claimed real
transcripts, or present these plain API probes as validation of temper's
future protocol adapters.
