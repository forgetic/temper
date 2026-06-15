use super::*;
use crate::workspace::RoleGitIdentity;

#[test]
fn loads_role_identities_from_env_for_distinct_roles() {
    let identities = role_identities_from_env(
        [
            "engineer".to_string(),
            "code-reviewer".to_string(),
            "engineer".to_string(),
        ],
        [
            (
                "TEMPER_FORGEJO_USER_ENGINEER".to_string(),
                " engineer-user ".to_string(),
            ),
            (
                "TEMPER_FORGEJO_TOKEN_ENGINEER".to_string(),
                " engineer-token ".to_string(),
            ),
            (
                "TEMPER_FORGEJO_USER_CODE_REVIEWER".to_string(),
                "reviewer-user".to_string(),
            ),
            (
                "TEMPER_FORGEJO_TOKEN_CODE_REVIEWER".to_string(),
                "reviewer-token".to_string(),
            ),
            (
                "TEMPER_FORGEJO_EMAIL_CODE_REVIEWER".to_string(),
                "reviewer@example.test".to_string(),
            ),
            (
                "TEMPER_FORGEJO_USER_ARCHITECT".to_string(),
                "ignored".to_string(),
            ),
            (
                "TEMPER_FORGEJO_TOKEN_ARCHITECT".to_string(),
                "ignored".to_string(),
            ),
        ],
    )
    .expect("identities load");

    assert_eq!(identities.len(), 2);
    assert_eq!(
        identities.get("engineer"),
        Some(&RoleGitIdentity {
            user: "engineer-user".to_string(),
            email: "engineer-user@noreply.localhost".to_string(),
            token: "engineer-token".to_string(),
        })
    );
    assert_eq!(
        identities.get("code-reviewer"),
        Some(&RoleGitIdentity {
            user: "reviewer-user".to_string(),
            email: "reviewer@example.test".to_string(),
            token: "reviewer-token".to_string(),
        })
    );
    assert!(!identities.contains_key("architect"));
}

#[test]
fn role_identity_errors_name_missing_user_or_token_and_role() {
    let missing_user = role_identities_from_env(
        ["engineer".to_string()],
        [(
            "TEMPER_FORGEJO_TOKEN_ENGINEER".to_string(),
            "token".to_string(),
        )],
    )
    .expect_err("missing user fails");
    assert!(missing_user.contains("TEMPER_FORGEJO_USER_ENGINEER"));
    assert!(missing_user.contains("role `engineer`"));

    let missing_token = role_identities_from_env(
        ["code-reviewer".to_string()],
        [
            (
                "TEMPER_FORGEJO_USER_CODE_REVIEWER".to_string(),
                "reviewer".to_string(),
            ),
            (
                "TEMPER_FORGEJO_TOKEN_CODE_REVIEWER".to_string(),
                " ".to_string(),
            ),
        ],
    )
    .expect_err("missing token fails");
    assert!(missing_token.contains("TEMPER_FORGEJO_TOKEN_CODE_REVIEWER"));
    assert!(missing_token.contains("role `code-reviewer`"));
}
