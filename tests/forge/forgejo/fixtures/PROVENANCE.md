# Forgejo document fixture provenance

The initial fixtures are hand-written selected-field examples derived from the retained v15 Swagger and tagged primary sources. They are not live HTTP transcripts. Round-trip tests generate documents with this crate and establish internal consistency only.

- Retained Swagger: `/home/free/src/rust/tmp/forgejo-swagger.json`, Forgejo API v1.
- [v15 webhook event names](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/modules/webhook/type.go): `HookEventType.Event` emits `pull_request_approved`, `pull_request_rejected`, `pull_request_comment`, and `action_run_failure`, `action_run_recover`, `action_run_success`.
- [v15 ActionPayload](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/modules/structs/hook.go): `ActionPayload` has `action`, `run`, `prior_status`, and optional `last_run`.
- [v15 ActionRun](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/modules/structs/action.go): `ActionRun` has `repository`, `trigger_user`, and `commit_sha`.
- [v15 action completion notifier](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/services/webhook/notifier.go): `ActionRunNowDone` emits success, recovery after a previous unsuccessful run, or failure.

The source-derived Actions fixture tests extraction only. Live Actions runner delivery remains unverified and is outside the isolated API conformance runner. No `workflow_run` or review-prefixed synthetic aliases are accepted.

The `v15/` directory contains actual isolated HTTP response bodies and whole webhook bodies, with version/binary SHA256/capture/isolation provenance. `observed.rs` decodes these production documents and verifies the captured whole-body HMAC signatures with the disposable fixture secret. Label exchange fixtures separately preserve unknown name/ID acceptance and name removal observations. These observations do not cover protected branches, inline review comments or Actions runners.
