use std::collections::{BTreeMap, BTreeSet};

use crate::workspace::RoleGitIdentity;

pub fn role_identities_from_env(
    roles: impl IntoIterator<Item = String>,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, RoleGitIdentity>, String> {
    let roles: BTreeSet<String> = roles.into_iter().collect();
    let vars: BTreeMap<String, String> = vars.into_iter().collect();
    let mut identities = BTreeMap::new();

    for role in roles {
        let key = env_role_key(&role);
        let user_var = format!("TEMPER_FORGEJO_USER_{key}");
        let token_var = format!("TEMPER_FORGEJO_TOKEN_{key}");
        let email_var = format!("TEMPER_FORGEJO_EMAIL_{key}");

        let user = required_env_value(&vars, &user_var, &role)?;
        let token = required_env_value(&vars, &token_var, &role)?;
        let email = vars
            .get(&email_var)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{user}@noreply.localhost"));

        identities.insert(role, RoleGitIdentity { user, email, token });
    }

    Ok(identities)
}

fn env_role_key(role: &str) -> String {
    role.chars()
        .flat_map(char::to_uppercase)
        .map(|character| {
            if character.is_ascii_uppercase() || character.is_ascii_digit() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn required_env_value(
    vars: &BTreeMap<String, String>,
    var_name: &str,
    role: &str,
) -> Result<String, String> {
    vars.get(var_name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!("no {var_name} in the environment for role `{role}`; is roles.env provisioned?")
        })
}
