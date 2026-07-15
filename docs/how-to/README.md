# How-to guides

How-to guides are focused recipes for specific tasks.

Current guides:

- [Iterate quickly during local development](fast-local-iteration.md)
- [End a development session cleanly](end-a-development-session.md)
- [Write Temper tests](write-temper-tests.md)
- [Run the reference delivery end-to-end scenarios](run-reference-delivery-end-to-end.md)
- [Run the cross-repo reference-delivery demo](run-cross-repo-reference-delivery-demo.md)
- [Run the daemon end-to-end fixture](run-daemon-e2e.md)
- [Find a post-merge validation report](post-merge-validation-report.md)
- [Verify implementation PR handoff end to end](verify-implementation-pr-handoff.md)
- [Configure Smith LLM responders from Temper](use-chatgpt-oauth-auth.md)
- [Configure a coding workspace external tool](configure-coding-workspace.md)
- [Operate durable agent traces and OpenTelemetry](operate-agent-traces.md)
- [Deploy Temper with systemd](deploy-with-systemd.md)
- Run the operator demo: `examples/reference-delivery/` (`./run.sh`) —
  deterministic jig-backed agents against a real throwaway Forgejo plus real
  host-mode forgejo-runner; see its
  [README](../../examples/reference-delivery/README.md).

Planned guides:

- Add a new Forge backend.
- Extend the Forge domain model safely.
- Write deterministic backend conformance tests.
- Map a provider-specific CI status into Temper CI states.
