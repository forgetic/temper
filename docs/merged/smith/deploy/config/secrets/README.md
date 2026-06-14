# `~/.config/smith/secrets/`

Live deployment secrets for the Smith worker tier. **Nothing real here is
tracked** — see `.gitignore`. The repo ships only this README, the `.gitignore`,
and `roles.env.example`.

| File | Who writes it | Contents |
| --- | --- | --- |
| `roles.env` | the Temper provisioner (`temper-provision-forgejo --out …`) | per-role + bot Forgejo credentials, mode `0600` |
| `roles.env.example` | tracked template | documents the shape `roles.env` must have |

`deploy/install.sh` never writes `roles.env` — that comes from provisioning
(`docs/how-to/provision-smith-dogfood.md`). `smith-worker` reads the per-role
git credentials (`TEMPER_FORGEJO_{USER,TOKEN}_<ROLEKEY>`, optional
`TEMPER_FORGEJO_EMAIL_<ROLEKEY>`) directly from this file via the unit's
`EnvironmentFile=` directive; no remapping shim is involved and secrets never
appear on a command line.

The Forgejo **API** tokens and the webhook HMAC secret belong to the Temper
daemon deployment (`~/.config/temper/secrets/`, see `temper/deploy/`), not to
this worker tier — workers never call the Forge API.
