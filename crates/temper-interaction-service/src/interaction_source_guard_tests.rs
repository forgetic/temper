use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_dir() {
        candidates.push(path);
    }
    if let Some(value) = std::env::var_os("CARGO_MANIFEST_DIR")
        && !value.is_empty() {
            candidates.push(PathBuf::from(value));
        }
    candidates
        .into_iter()
        .find_map(|start| {
            start
                .ancestors()
                .find(|dir| {
                    dir.join("Cargo.toml").is_file()
                        && dir.join("crates/temper-interaction-service").is_dir()
                })
                .map(Path::to_path_buf)
        })
        .expect("workspace root resolves")
}

#[test]
fn crates_sources_do_not_reintroduce_concrete_interaction_profiles() {
    let crates_dir = workspace_root().join("crates");
    let mut hits = Vec::new();
    scan_dir(&crates_dir, &mut hits);

    assert!(
        hits.is_empty(),
        concat!(
            "concrete interactive profile references must stay in tests, fixtures, ",
            "docs, examples, or plans:\n{}"
        ),
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
