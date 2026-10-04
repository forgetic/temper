# Isolated Forgejo v15 conformance

This opt-in check is outside default/fuzzy test budgets. It runs only
`/tmp/temper-forgejo-15/forgejo-15.0.0-linux-amd64`, creates a fresh temporary
work directory and SQLite database, binds HTTP and the webhook receiver to
loopback, and destroys the scratch instance after capture. It never invokes
the production `forgejo` wrapper or opens existing Forgejo state.

```sh
python3 tools/forgejo-conformance/run.py --output /tmp/temper-forgejo-observations
```

The output identifies the binary version and SHA256, capture time and
isolation. HTTP request bodies and response documents are actual observations;
authorization headers, passwords and API token creation responses are excluded.
Webhook documents are captured after the receiver queues its 204. The synthetic
hook secret is fixed and used only for these disposable fixtures.

Actions runner deliveries and protected-branch behavior remain unverified.
Tagged-source fixtures state their separate provenance. This runner must not
turn generated codec round-trips into claimed real transcripts.
