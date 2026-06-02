use std::fs;
use std::path::{Path, PathBuf};

const LEGACY_PROMPT_FILES: &[&str] = &[
    "architect.md",
    "engineer.md",
    "reviewer.md",
    "owner.md",
    "human.md",
];

const LEGACY_PROMPT_CONSTANTS: &[&str] = &[
    "ARCHITECT_SYSTEM_PROMPT",
    "ENGINEER_SYSTEM_PROMPT",
    "REVIEWER_SYSTEM_PROMPT",
    "OWNER_SYSTEM_PROMPT",
    "HUMAN_SYSTEM_PROMPT",
];

#[test]
fn production_agents_do_not_ship_legacy_workflow_role_prompts() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let prompt_dir = manifest_dir.join("src/prompts");

    for file in LEGACY_PROMPT_FILES {
        assert!(
            !prompt_dir.join(file).exists(),
            "production temper-agents must not ship checked-in workflow-role prompt file {file}"
        );
    }
    assert!(
        prompt_dir.join("product_manager.md").exists(),
        "non-workflow product-manager conversational prompt remains allowed"
    );
}

#[test]
fn production_agents_do_not_import_legacy_prompt_constants() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for file in rust_files(&src) {
        let text = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("reading {} failed: {error}", file.display()));
        for constant in LEGACY_PROMPT_CONSTANTS {
            if text.contains(constant) {
                hits.push(format!("{} contains {constant}", file.display()));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "production temper-agents imports legacy workflow prompt constants:\n{}",
        hits.join("\n")
    );
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_files(root, &mut files);
    files.sort();
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("reading directory {} failed: {error}", dir.display()))
    {
        let entry = entry.expect("directory entry is readable");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
