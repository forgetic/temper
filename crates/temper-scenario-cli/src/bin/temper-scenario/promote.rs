// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EX_USAGE: u8 = 64;

const USAGE: &str = "\
Draft an optional scenario-promotion candidate from a validation artifact.

Usage: temper-scenario promote <VALIDATION_ARTIFACT> [--name <SLUG>] [--output-dir <DIR>]

Arguments:
  VALIDATION_ARTIFACT  Validation report file or artifact directory to cite as the source proof

Options:
  --name, --slug <SLUG> Intended scenario name/slug for the candidate draft
  --output-dir <DIR>    Directory for the candidate Markdown draft (default: current directory)
  -h, --help            Print help

This scaffold only writes a deterministic operator prompt for post-validation
promotion. For a feature/plan scenario, use `temper-scenario scaffold`; that
command writes a checked-in scenario-ready inherited bundle with local Jig data
instead of a Markdown-only candidate. It does not create
Forgejo issues or PRs. Promotion is
optional follow-up work: the validation report remains the required artifact,
and promoted scenarios should preserve stable intended behavior rather than
incidental implementation details.";

pub(super) fn command(args: &[String]) -> ExitCode {
    let args = match parse_args(args) {
        Ok(ParseResult::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    let source = match inspect_source(&args.source) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("temper-scenario promote: {error}");
            return ExitCode::FAILURE;
        }
    };

    let candidate = CandidateDraft::from_args(&args, source);
    let output_path = args
        .output_dir
        .join(format!("scenario-candidate-{}.md", candidate.slug));

    if let Err(error) = write_draft(&output_path, &candidate.render_markdown()) {
        eprintln!("temper-scenario promote: {error}");
        return ExitCode::FAILURE;
    }

    println!("{}", output_path.display());
    ExitCode::SUCCESS
}

#[derive(Debug)]
struct Args {
    source: PathBuf,
    supplied_name: Option<String>,
    output_dir: PathBuf,
}

#[derive(Debug)]
enum ParseResult {
    Help,
    Args(Args),
}

#[derive(Debug)]
struct SourceArtifact {
    path: PathBuf,
    kind: SourceKind,
    inferred_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum SourceKind {
    ReportFile,
    ArtifactDirectory,
}

impl SourceKind {
    fn label(self) -> &'static str {
        match self {
            Self::ReportFile => "validation report file",
            Self::ArtifactDirectory => "validation artifact directory",
        }
    }
}

#[derive(Debug)]
struct CandidateDraft {
    source: SourceArtifact,
    slug: String,
    name_source: NameSource,
}

#[derive(Debug)]
enum NameSource {
    Supplied(String),
    InferredFromReport,
    InferredFromPath,
}

impl NameSource {
    fn description(&self) -> String {
        match self {
            Self::Supplied(raw) => format!("supplied from `{raw}`"),
            Self::InferredFromReport => "inferred from validation report content".to_string(),
            Self::InferredFromPath => "inferred from validation artifact path".to_string(),
        }
    }
}

impl CandidateDraft {
    fn from_args(args: &Args, source: SourceArtifact) -> Self {
        let (slug, name_source) = match args.supplied_name.as_deref() {
            Some(name) => (scenario_slug(name), NameSource::Supplied(name.to_string())),
            None => match source.inferred_name.as_deref() {
                Some(name) => (scenario_slug(name), NameSource::InferredFromReport),
                None => (
                    scenario_slug(&fallback_name_from_path(&source.path)),
                    NameSource::InferredFromPath,
                ),
            },
        };

        Self {
            source,
            slug,
            name_source,
        }
    }

    fn render_markdown(&self) -> String {
        format!(
            "# Scenario promotion candidate: {slug}\n\
\n\
- Source validation artifact: `{source_path}`\n\
- Source artifact kind: {source_kind}\n\
- Intended scenario name/slug: `{slug}` ({name_source})\n\
\n\
## Promotion rationale\n\
\n\
TODO: Explain why this validation proof captures stable intended behavior worth promoting.\n\
\n\
## Stable behavior to preserve\n\
\n\
TODO: Describe the externally observable workflow behavior that should become reusable regression coverage.\n\
\n\
## Fixture notes\n\
\n\
TODO: List the minimal fixture files, seed state, and assertions needed to reproduce the proof.\n\
\n\
## Promotion boundaries\n\
\n\
- Scenario promotion is optional follow-up work; the validation report remains the required artifact.\n\
- Preserve only stable intended behavior from the validation proof.\n\
- Do not encode incidental implementation details, generated logs, runtime state, credentials, or provider-specific accidents.\n\
- This scaffold does not create Forgejo issues or PRs and does not generate a complete scenario.\n",
            slug = self.slug,
            source_path = super::display_path(&self.source.path),
            source_kind = self.source.kind.label(),
            name_source = self.name_source.description(),
        )
    }
}

fn parse_args(args: &[String]) -> Result<ParseResult, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseResult::Help);
    }

    let mut source = None;
    let mut supplied_name = None;
    let mut output_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--name" | "--slug" => {
                let flag = args[index].as_str();
                let value = flag_value(args, index, flag)?;
                if value.trim().is_empty() {
                    eprintln!("temper-scenario promote: {flag} must not be empty\n\n{USAGE}");
                    return Err(());
                }
                if supplied_name.replace(value.to_string()).is_some() {
                    eprintln!("temper-scenario promote: duplicate {flag} option\n\n{USAGE}");
                    return Err(());
                }
                index += 2;
            }
            "--output-dir" => {
                let value = flag_value(args, index, "--output-dir")?;
                if output_dir.replace(PathBuf::from(value)).is_some() {
                    eprintln!("temper-scenario promote: duplicate --output-dir option\n\n{USAGE}");
                    return Err(());
                }
                index += 2;
            }
            other if other.starts_with("--") => {
                eprintln!("temper-scenario promote: unexpected option `{other}`\n\n{USAGE}");
                return Err(());
            }
            other => {
                if source.replace(PathBuf::from(other)).is_some() {
                    eprintln!("temper-scenario promote: unexpected argument `{other}`\n\n{USAGE}");
                    return Err(());
                }
                index += 1;
            }
        }
    }

    let Some(source) = source else {
        eprintln!("temper-scenario promote: missing VALIDATION_ARTIFACT\n\n{USAGE}");
        return Err(());
    };

    Ok(ParseResult::Args(Args {
        source,
        supplied_name,
        output_dir: output_dir.unwrap_or_else(|| PathBuf::from(".")),
    }))
}

fn flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, ()> {
    let Some(value) = args.get(index + 1) else {
        eprintln!("temper-scenario promote: {flag} requires a value\n\n{USAGE}");
        return Err(());
    };
    if value.starts_with("--") {
        eprintln!("temper-scenario promote: {flag} requires a value\n\n{USAGE}");
        return Err(());
    }
    Ok(value)
}

fn inspect_source(path: &Path) -> Result<SourceArtifact, String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "validation artifact path is missing or unusable: {}: {error}",
            super::display_path(path)
        )
    })?;

    if metadata.is_file() {
        let source = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read validation report file {}: {error}",
                super::display_path(path)
            )
        })?;
        Ok(SourceArtifact {
            path: path.to_path_buf(),
            kind: SourceKind::ReportFile,
            inferred_name: infer_name_from_report(&source),
        })
    } else if metadata.is_dir() {
        fs::read_dir(path).map_err(|error| {
            format!(
                "failed to read validation artifact directory {}: {error}",
                super::display_path(path)
            )
        })?;
        Ok(SourceArtifact {
            path: path.to_path_buf(),
            kind: SourceKind::ArtifactDirectory,
            inferred_name: None,
        })
    } else {
        Err(format!(
            "validation artifact path is not a file or directory: {}",
            super::display_path(path)
        ))
    }
}

fn infer_name_from_report(source: &str) -> Option<String> {
    source
        .lines()
        .find_map(|line| extract_backticked_after(line, "scenario: `"))
        .or_else(|| {
            source
                .lines()
                .find_map(|line| extract_backticked_after(line, "Scenario `"))
        })
}

fn extract_backticked_after(line: &str, marker: &str) -> Option<String> {
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find('`')?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn fallback_name_from_path(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("scenario-candidate")
        .to_string()
}

fn scenario_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_was_separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator && !slug.is_empty() {
            slug.push('-');
            previous_was_separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "scenario-candidate".to_string()
    } else {
        slug
    }
}

fn write_draft(path: &Path, markdown: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!("draft path has no parent: {}", path.display()));
    };
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create output directory {}: {error}",
            parent.display()
        )
    })?;
    fs::write(path, markdown)
        .map_err(|error| format!("failed to write draft {}: {error}", path.display()))
}
