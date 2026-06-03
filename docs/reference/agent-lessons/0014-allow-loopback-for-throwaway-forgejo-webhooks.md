# Lesson 0014: Allow loopback for throwaway Forgejo webhooks

## Tags

`forgejo`, `webhook`, `testing`, `configuration`

## Trigger

The first live run of the long-poll webhook wakeup regression registered a repo
webhook pointing at `http://127.0.0.1:<port>/forgejo/webhook`, but no webhook
requests reached the trigger. Workers slept for the full long-poll window and
the test timed out with no accepted webhook logs.

## What went wrong

The throwaway Forgejo fixture did not explicitly allow loopback webhook targets.
Forgejo accepted the hook registration, but deliveries to the local trigger were
blocked before any request reached the test process, making the failure look like
a worker wake problem.

## Steering for future agents

When a throwaway Forgejo must call a host-local webhook receiver, put a
`[webhook]` section in `app.ini` with an explicit loopback allow-list, for
example:

```ini
[webhook]
ALLOWED_HOST_LIST = 127.0.0.1,localhost
```

Do this in both test fixtures and demo launchers. If a webhook test sees no
requests at the trigger even though hook registration succeeded, check this
setting before debugging signatures or worker wake sockets.

## Where this is now documented

- `crates/temper-forgejo-fixture/src/lib.rs` writes the loopback
  `ALLOWED_HOST_LIST` into the e2e fixture config.
- `examples/reference-delivery/run.sh` writes the same setting for the demo.
- `crates/temper-testing/tests/forgejo_webhook_wakeup.rs` live-validates the
  path by requiring accepted Forgejo webhooks to wake long-poll workers.
