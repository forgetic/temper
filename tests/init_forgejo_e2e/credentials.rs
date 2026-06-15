use std::collections::BTreeMap;
use std::path::Path;

use super::ADMIN_USER;

pub(super) fn read_admin_token(credentials_path: &Path) -> String {
    let value = parse_credentials(credentials_path);
    value["forge"]["users"][ADMIN_USER]["token"]
        .as_str()
        .expect("admin token in credentials")
        .to_string()
}

pub(super) fn role_tokens(credentials_path: &Path) -> BTreeMap<String, String> {
    let value = parse_credentials(credentials_path);
    let mut out = BTreeMap::new();
    if let Some(users) = value["forge"]["users"].as_table() {
        for (name, user) in users {
            if name == ADMIN_USER {
                continue;
            }
            if let Some(token) = user.get("token").and_then(toml::Value::as_str) {
                out.insert(name.clone(), token.to_string());
            }
        }
    }
    out
}

fn parse_credentials(credentials_path: &Path) -> toml::Value {
    let text = std::fs::read_to_string(credentials_path).expect("read credentials.toml");
    toml::from_str::<toml::Value>(&text).expect("credentials.toml parses as TOML")
}
