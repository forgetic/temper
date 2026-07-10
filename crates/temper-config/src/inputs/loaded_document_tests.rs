// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use crate::{
    CredentialSource, CredentialSourceKind, CredentialSourceOrigin, EnvMap, ExposeSecret,
    LoadInputs, NoEnv, PathResolver, load_documents_explicit,
};

const MINIMAL_CONFIG: &str =
    "schema_version = 1\n[engine]\nrepos = [\"a/b\"]\nroles = [\"engineer\"]\n";

fn temp_dir(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "temper-loaded-documents-{tag}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn reports_every_credential_source_origin_and_kind() {
    let dir = temp_dir("origins");
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");

    let explicit_file = dir.join("explicit.toml");
    std::fs::write(&explicit_file, "schema_version = 1\n").expect("explicit credentials");
    let explicit = load_documents_explicit(&LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: Some(explicit_file.clone()),
        env: &NoEnv,
        paths: &PathResolver::default(),
    })
    .expect("explicit source");
    assert_eq!(
        explicit.credential_source,
        Some(CredentialSource {
            path: explicit_file,
            kind: CredentialSourceKind::File,
            origin: CredentialSourceOrigin::Explicit,
        })
    );

    let ambient_dir = dir.join("ambient");
    std::fs::create_dir_all(&ambient_dir).expect("ambient dir");
    let mut ambient_env = EnvMap::new();
    ambient_env.insert(
        "CREDENTIALS_DIRECTORY",
        ambient_dir.to_string_lossy().into_owned(),
    );
    let ambient = load_documents_explicit(&LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &ambient_env,
        paths: &PathResolver::default(),
    })
    .expect("ambient source");
    assert_source(
        &ambient,
        CredentialSourceKind::Directory,
        CredentialSourceOrigin::CredentialsDirectory,
        &ambient_dir,
    );

    let sibling_path = dir.join("credentials.toml");
    std::fs::write(&sibling_path, "schema_version = 1\n").expect("sibling credentials");
    let sibling = load_documents_explicit(&LoadInputs {
        explicit_config: Some(config_path),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    })
    .expect("sibling source");
    assert_source(
        &sibling,
        CredentialSourceKind::File,
        CredentialSourceOrigin::ConfigSibling,
        &sibling_path,
    );

    let xdg = dir.join("xdg");
    let default_root = xdg.join("temper");
    std::fs::create_dir_all(&default_root).expect("default root");
    std::fs::write(default_root.join("config.toml"), MINIMAL_CONFIG).expect("default config");
    let default_credentials = default_root.join("credentials.toml");
    std::fs::write(&default_credentials, "schema_version = 1\n").expect("default credentials");
    let default = load_documents_explicit(&LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver {
            xdg_config_home: Some(xdg),
            ..PathResolver::default()
        },
    })
    .expect("default source");
    assert_source(
        &default,
        CredentialSourceKind::File,
        CredentialSourceOrigin::Default,
        &default_credentials,
    );

    let _ = std::fs::remove_dir_all(dir);
}

fn assert_source(
    documents: &crate::LoadedDocuments,
    kind: CredentialSourceKind,
    origin: CredentialSourceOrigin,
    path: &std::path::Path,
) {
    assert_eq!(
        documents.credential_source.as_ref().map(|source| (
            source.kind,
            source.origin,
            source.path.as_path()
        )),
        Some((kind, origin, path))
    );
}

#[test]
fn merges_directory_toml_and_named_files_and_redacts_debug() {
    let dir = temp_dir("directory-merge");
    let config_path = dir.join("config.toml");
    let secrets_dir = dir.join("secrets");
    std::fs::create_dir_all(&secrets_dir).expect("secrets dir");
    std::fs::write(
        &config_path,
        "schema_version = 1\n[engine]\nforge_token = \"forge-token\"\n",
    )
    .expect("config");
    std::fs::write(
        secrets_dir.join("credentials.toml"),
        "schema_version = 1\n[forge.users.root]\npassword = \"toml-password\"\n[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"provider-secret\"\n",
    )
    .expect("credentials");
    std::fs::write(secrets_dir.join("forge-token"), "named-token\n").expect("named secret");

    let documents = load_documents_explicit(&LoadInputs {
        explicit_config: Some(config_path),
        explicit_credentials: Some(secrets_dir),
        env: &NoEnv,
        paths: &PathResolver::default(),
    })
    .expect("directory documents");

    assert!(documents.credentials.forge.users.contains_key("root"));
    assert!(
        documents
            .credentials
            .agent
            .providers
            .contains_key("deepseek")
    );
    assert!(
        documents
            .credentials
            .named_files
            .contains_key("forge-token")
    );
    assert_eq!(
        documents
            .resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("named-token")
    );
    assert_eq!(
        documents
            .credential_source
            .as_ref()
            .map(|source| (source.kind, source.origin)),
        Some((
            CredentialSourceKind::Directory,
            CredentialSourceOrigin::Explicit,
        ))
    );
    let debug = format!("{documents:?}");
    for secret in ["toml-password", "provider-secret", "named-token"] {
        assert!(!debug.contains(secret), "secret leaked in Debug: {debug}");
    }
    assert!(debug.contains("[REDACTED]"), "{debug}");

    let _ = std::fs::remove_dir_all(dir);
}
