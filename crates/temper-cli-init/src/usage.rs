// SPDX-License-Identifier: MPL-2.0

//! Command help for `temper init`.

/// `temper init [OPTIONS]` usage.
pub const USAGE: &str = r#"Interactively configure a temper deployment.

Walks you through your forge URL, admin credentials, and LLM provider choice, then
writes config.toml + workflow.yaml + a webhook secret + credentials.toml.
Forge-side provisioning (repo/users/labels/webhook registration) only runs when
--apply is set; --yes skips that deployment-wide apply confirmation.

Usage: temper [GLOBAL OPTIONS] init [OPTIONS]

Options:
  --force                       Overwrite existing local files
  --apply                       After writing local files, provision the forge
  --yes                         With --apply, skip the provisioning confirmation
  --existing-repo               Supported compatibility behavior: require every
                                selected repo to already exist when provisioning
  --topology      <standalone|distributed>
                                Topology to collect for the initialized bundle
  --repo          <owner/name>  Managed repository to provision (repeatable)
  --workflow      <builtin|PATH>  Builtin workflow name or JSON/YAML workflow file
  --forge         <URL>         Forgejo URL; skips the Forge URL prompt
  --bind          <ADDR>        Daemon bind / webhook advertise address override
  --workspace     <PATH>        Top-level worker workspace root to write
  --provider      <anthropic|chatgpt|deepseek|none>
                                LLM provider profile to configure
  --provider-url  <URL>         Base URL override for the provider
  --answers       <FILE>        TOML answers file; implies --non-interactive
  --non-interactive             Run without prompts; all required values must
                                be supplied via flags, --answers, or environment
  --admin-user   <VALUE>        Forgejo admin username; skips the admin prompt
  -h, --help                    Print help

Environment variables (only honoured with --non-interactive or --answers):
  TEMPER_INIT_ADMIN_PASSWORD    Forgejo admin password (wins over --answers)
  TEMPER_INIT_PROVIDER_KEY      DeepSeek provider API key (wins over --answers)

Answers file (TOML, used by --answers and implies --non-interactive):
  schema_version = 1
  topology = "standalone"          # or "distributed"
  forge_url = "http://localhost:3000"
  workflow = "basic-delivery"      # builtin or JSON/YAML path
  webhook_addr = "http://127.0.0.1:8314"
  admin_user = "root"
  admin_password = "..."           # secret; env TEMPER_INIT_ADMIN_PASSWORD wins
  provider = "deepseek"            # anthropic|chatgpt|deepseek|none
  provider_key = "..."             # secret; env TEMPER_INIT_PROVIDER_KEY wins
  provider_url = "http://localhost:9999/v1"
  repos = ["owner/name", "owner/other"]

The answers file cannot set --apply; pass --apply explicitly to provision."#;
