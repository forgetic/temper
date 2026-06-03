use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn crates_sources_do_not_reintroduce_concrete_interaction_profiles() {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate has a crates parent")
        .to_path_buf();
    let mut hits = Vec::new();
    scan_dir(&crates_dir, &mut hits);

    assert!(
        hits.is_empty(),
        "concrete interactive profile references must stay in tests, fixtures, docs, examples, or plans:\n{}",
        hits.join("\n")
    );
}

fn scan_dir(dir: &Path, hits: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("scan crates directory") {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        if should_skip(&path) {
            continue;
        }
        if path.is_dir() {
            scan_dir(&path, hits);
        } else if path.is_file() {
            scan_file(&path, hits);
        }
    }
}

fn should_skip(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name == "fixtures" || name == "tests" || name == "target" || name.contains("test")
    })
}

fn scan_file(path: &Path, hits: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for (index, line) in text.lines().enumerate() {
        if let Some(pattern) = FORBIDDEN.iter().find(|pattern| line.contains(**pattern)) {
            hits.push(format!(
                "{}:{}: matched `{}`",
                path.display(),
                index + 1,
                pattern
            ));
        }
    }
}

const FORBIDDEN: &[&str] = &[
    "product-manager",
    "ProductChat",
    "product_chat",
    "product-chat",
    "Product conversation",
    "TEMPER_PRODUCT_CHAT",
    "/file ",
];
