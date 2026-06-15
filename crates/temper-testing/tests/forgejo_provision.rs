//! Phase 2 provisioning + identity test against a real Forgejo.
//!
//! `#[ignore]`d, so the default `cargo test` never downloads a binary, opens a
//! socket, or spawns a server. No extra environment variable is required; run it
//! with:
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_provision -- --ignored
//! ```
//!
//! It starts a [`ForgejoServer`] from the declared cached provisioned state and
//! proves the Phase 2 contract end to end against the real backend:
//!
//! - admin bootstrap works (a usable admin token comes back);
//! - each role token resolves to the matching `current_user` via `ForgejoForge`;
//! - the `auto_init` repository resolves by owner/name path;
//! - every label the compiled workflow declares exists on the repo;
//! - the CI workflow file is committed and `has_actions` is on.
//!
//! `#[tokio::test]` because `ForgejoForge` is async (a real HTTP reactor); the
//! provisioning REST calls and the per-role identity checks all run under it.

use std::collections::BTreeSet;
use temper_forge_model::RepositoryPath;
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_server::start_cached_provisioned_server;
use temper_testing::{runner_config, workflow};

#[test]
#[ignore = "boots a real Forgejo server; run with --ignored"]
fn provisions_identity_repo_labels_and_workflow() {
    temper_engine_io::block_on(async move {
        // The cached Forgejo fixture uses a *blocking* reqwest client for readiness
        // polling; building/dropping that client inside the async test context trips
        // Tokio's "cannot drop a runtime in an async context" guard. Boot it on a
        // blocking thread so the nested blocking runtime lives and dies off-reactor.
        let cached = skein::runtime::spawn_blocking(start_cached_provisioned_server)
            .await
            .expect("forgejo cached provisioned state starts");
        let server = cached.server;
        let provisioned = cached.provisioned;
        let base = server.base_url().to_string();

        // 1. Admin bootstrap produced a non-empty token.
        assert!(
            !provisioned.admin_token.is_empty(),
            "admin token should be non-empty"
        );

        // 2. Every role in the runner config has an identity, and each token
        //    resolves to the matching `current_user`.
        let config = runner_config();
        assert_eq!(
            provisioned.roles.len(),
            config.role_bindings.len(),
            "one identity per role binding"
        );
        for binding in &config.role_bindings {
            let identity = provisioned
                .role(&binding.role)
                .unwrap_or_else(|| panic!("role {:?} provisioned", binding.role));
            assert_eq!(
                identity.user, binding.user.handle,
                "role login matches the runner-config binding"
            );

            let forge = ForgejoForge::new(ForgejoConfig::new(&base, &identity.token));
            let current = forge
                .current_user()
                .await
                .expect("role token resolves a current user");
            assert_eq!(
                current.handle, identity.user,
                "token's current_user is the role's user"
            );
        }

        // 3. The auto-initialized repository resolves by path.
        let admin = ForgejoForge::new(ForgejoConfig::new(&base, &provisioned.admin_token));
        let path = RepositoryPath::new(&provisioned.owner, &provisioned.name);
        let repo = admin
            .get_repository_by_path(&path)
            .await
            .expect("repo lookup succeeds")
            .expect("auto_init repo resolves by path");
        assert_eq!(
            repo.id, provisioned.repository,
            "returned RepositoryId resolves by path"
        );

        // 4. Every label the compiled workflow declares exists on the repo.
        let declared: BTreeSet<String> = workflow()
            .compile()
            .labels()
            .labels()
            .iter()
            .map(|spec| spec.id.to_string())
            .collect();
        let present: BTreeSet<String> = admin
            .list_labels(&provisioned.repository)
            .await
            .expect("list labels succeeds")
            .into_iter()
            .map(|label| label.name)
            .collect();
        assert!(
            declared.is_subset(&present),
            "missing labels: {:?}",
            declared.difference(&present).collect::<Vec<_>>()
        );

        // 5. The CI workflow file is committed and Actions are enabled.
        let client = temper_engine_io::http::JsonClient::new();
        let (contents_status, _) = client
            .send_expect_json(
                "GET",
                format!(
                    "{base}/api/v1/repos/{}/{}/contents/.forgejo/workflows/ci.yml",
                    provisioned.owner, provisioned.name
                ),
                Some(&provisioned.admin_token),
                None,
                "contents request",
            )
            .await;
        assert!(
            (200..300).contains(&contents_status),
            "workflow file should be committed, got {contents_status}"
        );

        let (_, repo_json) = client
            .send_expect_json(
                "GET",
                format!(
                    "{base}/api/v1/repos/{}/{}",
                    provisioned.owner, provisioned.name
                ),
                Some(&provisioned.admin_token),
                None,
                "repo request",
            )
            .await;
        assert_eq!(
            repo_json["has_actions"].as_bool(),
            Some(true),
            "has_actions should be enabled"
        );

        // Tear down explicitly so any panic in drop surfaces here.
        drop(server);
    })
}
