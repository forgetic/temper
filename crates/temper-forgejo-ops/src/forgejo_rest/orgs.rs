//! Organization, team, and user provisioning operations.

use serde_json::{Value, json};

use super::client::{Auth, Client, RestError, Result, accept_or_conflict, json_ok};

/// The shared password assigned to demo role users.
pub const ROLE_PASSWORD: &str = "R0le-Phase2-e2e!";

/// Token scopes role workers need for the reference-delivery demo.
const TOKEN_SCOPES: &[&str] = &[
    "write:repository",
    "write:issue",
    "write:user",
    "read:organization",
];

pub async fn ensure_org(client: &Client, base: &str, token: &str, owner: &str) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/orgs"),
            Auth::Token(token),
            Some(&json!({ "username": owner })),
        )
        .await?;
    accept_or_conflict(resp, "create org")
}

pub async fn owners_team_id(client: &Client, base: &str, token: &str, owner: &str) -> Result<i64> {
    let resp = client
        .send(
            "GET",
            format!("{base}/api/v1/orgs/{owner}/teams"),
            Auth::Token(token),
            None,
        )
        .await?;
    let teams: Value = json_ok(resp, "list org teams")?;
    teams
        .as_array()
        .and_then(|teams| {
            teams
                .iter()
                .find(|team| team["name"].as_str() == Some("Owners"))
        })
        .and_then(|team| team["id"].as_i64())
        .ok_or_else(|| RestError::Shape {
            what: "owners team".into(),
            detail: "no Owners team in org teams response".into(),
        })
}

pub async fn create_user(
    client: &Client,
    base: &str,
    token: &str,
    login: &str,
    email: &str,
) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/admin/users"),
            Auth::Token(token),
            Some(&json!({
                "username": login,
                "email": email,
                "password": ROLE_PASSWORD,
                "must_change_password": false,
            })),
        )
        .await?;
    accept_or_conflict(resp, "create user")
}

pub async fn add_team_member(
    client: &Client,
    base: &str,
    token: &str,
    team_id: i64,
    login: &str,
) -> Result<()> {
    let resp = client
        .send(
            "PUT",
            format!("{base}/api/v1/teams/{team_id}/members/{login}"),
            Auth::Token(token),
            None,
        )
        .await?;
    accept_or_conflict(resp, "add team member")
}

pub async fn mint_user_token(client: &Client, base: &str, login: &str) -> Result<String> {
    // Forgejo token names are user-unique. The reference demo may run the
    // provisioner once per configured repository, reusing the same role users;
    // give each minted token a unique, non-secret name so repeated provisioning
    // does not fail with a duplicate-token conflict.
    let token_name = format!(
        "temper-{login}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/users/{login}/tokens"),
            Auth::Basic(login, ROLE_PASSWORD),
            Some(&json!({
                "name": token_name,
                "scopes": TOKEN_SCOPES,
            })),
        )
        .await?;
    let body: Value = json_ok(resp, "mint user token")?;
    body["sha1"]
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| RestError::Shape {
            what: "user token".into(),
            detail: "no non-empty sha1 in token response".into(),
        })
}
