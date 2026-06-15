use super::*;

#[test]
fn intake_author_role_variant_round_trips() {
    let mut spec = valid_spec();
    spec.intake_author = Some(RawIntakeAuthor::Role {
        role: "engineer".to_string(),
    });

    let workflow = spec
        .validate()
        .expect("intake author referencing a declared role validates");
    assert_eq!(
        workflow.intake_author(),
        Some(&IntakeAuthor::Role("engineer".into()))
    );
}

#[test]
fn intake_author_site_admin_variant_round_trips() {
    let mut spec = valid_spec();
    spec.intake_author = Some(RawIntakeAuthor::SiteAdmin);

    let workflow = spec.validate().expect("site_admin intake author validates");
    assert_eq!(workflow.intake_author(), Some(&IntakeAuthor::SiteAdmin));
}

#[test]
fn intake_author_defaults_to_none() {
    let workflow = valid_spec()
        .validate()
        .expect("spec without intake author validates");
    assert_eq!(workflow.intake_author(), None);
}

#[test]
fn intake_author_undeclared_role_is_diagnosed() {
    let mut spec = valid_spec();
    spec.intake_author = Some(RawIntakeAuthor::Role {
        role: "ghost".to_string(),
    });

    let errors = spec
        .validate()
        .expect_err("intake author referencing an undeclared role must fail");
    assert!(
        errors
            .diagnostics()
            .contains(&Diagnostic::UndeclaredReference {
                expected: SymbolKind::Role,
                id: "ghost".to_string(),
                site: ReferenceSite::IntakeAuthor,
            })
    );
}

#[test]
fn intake_author_parses_from_json() {
    let role_json = r#"{ "kind": "role", "role": "human" }"#;
    let role: RawIntakeAuthor = serde_json::from_str(role_json).expect("role form parses");
    assert_eq!(
        role,
        RawIntakeAuthor::Role {
            role: "human".to_string()
        }
    );

    let admin_json = r#"{ "kind": "site_admin" }"#;
    let admin: RawIntakeAuthor = serde_json::from_str(admin_json).expect("site_admin form parses");
    assert_eq!(admin, RawIntakeAuthor::SiteAdmin);
}
