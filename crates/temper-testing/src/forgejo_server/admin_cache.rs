//! Cached bare Forgejo state with only a site-admin account.
//!
//! `temper init` end-to-end tests need to exercise all project provisioning
//! themselves, so they cannot reuse the richer reference-delivery caches in
//! [`super::provision_cache`]. This fixture caches just Forgejo's first-start
//! migrations plus one reusable site admin. Each caller still receives an
//! isolated `/tmp` copy of that bare/admin data tree via
//! [`ForgejoServer::start_with_state`].

use super::{ForgejoServer, ForgejoState, ServerError};
use serde::{Deserialize, Serialize};

/// Metadata stored with the bare/admin Forgejo state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BareAdmin {
    /// Login name for the cached site admin.
    pub username: String,
    /// Email configured for the cached site admin.
    pub email: String,
}

/// A per-test Forgejo process restored from the cached bare/admin state.
pub struct CachedBareAdminServer {
    /// Running Forgejo server backed by a fresh `/tmp` copy of cached state.
    pub server: ForgejoServer,
    /// Site-admin identity present in the cached state.
    pub admin: BareAdmin,
    /// Whether this call reused an existing `.cache` tree.
    pub cache_hit: bool,
    /// Stable state-cache key, for diagnostics.
    pub cache_key: String,
}

/// Starts a per-test Forgejo from a cached state containing no project state:
/// only Forgejo's initialized data tree plus the requested site admin.
pub fn start_cached_bare_admin_server(
    username: &str,
    password: &str,
    email: &str,
) -> Result<CachedBareAdminServer, ServerError> {
    let admin = BareAdmin {
        username: username.to_string(),
        email: email.to_string(),
    };
    let state = ForgejoState::new(bare_admin_state_description(username, password, email))?;
    let cached = ForgejoServer::start_with_state(&state, |server| {
        create_site_admin(server, username, password, email)?;
        Ok::<BareAdmin, ServerError>(admin)
    })?;
    Ok(CachedBareAdminServer {
        server: cached.server,
        admin: cached.metadata,
        cache_hit: cached.cache_hit,
        cache_key: cached.cache_key,
    })
}

fn bare_admin_state_description(username: &str, password: &str, email: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "temper-bare-admin-forgejo",
        "version": 1,
        "admin_user": username,
        "admin_email": email,
        "admin_password_sha256": sha256_hex(password.as_bytes()),
    })
}

fn create_site_admin(
    server: &ForgejoServer,
    username: &str,
    password: &str,
    email: &str,
) -> Result<(), ServerError> {
    match server.run_cli(&[
        "admin",
        "user",
        "create",
        "--username",
        username,
        "--password",
        password,
        "--email",
        email,
        "--admin",
        "--must-change-password=false",
    ]) {
        Ok(_) => Ok(()),
        Err(error) if error.to_string().to_lowercase().contains("exist") => Ok(()),
        Err(error) => Err(error),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_description_keys_admin_identity_without_plaintext_password() {
        let json = bare_admin_state_description("initadmin", "secret-password", "admin@example.invalid");
        assert_eq!(json["kind"], "temper-bare-admin-forgejo");
        assert_eq!(json["admin_user"], "initadmin");
        assert_eq!(json["admin_email"], "admin@example.invalid");
        assert_ne!(json["admin_password_sha256"], "secret-password");
        assert_eq!(
            json["admin_password_sha256"].as_str().map(str::len),
            Some(64)
        );
    }
}
