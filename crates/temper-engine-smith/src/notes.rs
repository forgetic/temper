//! Smith's JSON shapes for jig's scoped note and recall tools.
//! The root checks bounds and authority before any value becomes durable.

use jig_core_notes as notes;
use skein_lib::List;

use crate::{Problem, json::Value};

pub(crate) fn new(value: &Value) -> Result<(notes::New, Option<u32>), Problem> {
    let recalled = match value.field(b"recalled")? {
        Some(revision) => Some(revision.small()?),
        None => None,
    };
    let name = match value.field(b"name")? {
        Some(name) => name.number()?,
        None => 0,
    };
    if (recalled.is_some() && name == 0) || (recalled.is_none() && name != 0) {
        return Err(Problem::Range);
    }
    let references = match value.field(b"references")? {
        Some(value) => numbers(value)?,
        None => List::with_capacity(0),
    };
    Ok((
        notes::New {
            name,
            scope: scope(value.required(b"scope")?)?,
            description: value.required(b"description")?.text()?,
            body: value.required(b"body")?.text()?,
            references,
            author: notes::Author::Task { task: 0, attempt: 0 },
        },
        recalled,
    ))
}

pub(crate) fn recall(value: &Value) -> Result<(notes::Recall, u32), Problem> {
    let page = match value.field(b"page")? {
        Some(page) => page.small()?,
        None => 0,
    };
    let by = match value.required(b"kind")?.text()?.as_ref() {
        b"name" => {
            let name = value.required(b"name")?.number()?;
            if name == 0 {
                return Err(Problem::Range);
            }
            notes::Recall::Name { name }
        }
        b"search" => {
            let values = value.required(b"scopes")?.array()?;
            let Ok(capacity) = u32::try_from(values.len()) else { return Err(Problem::TooLarge) };
            let mut scopes = List::with_capacity(capacity);
            for value in values {
                if scopes.push(scope(value)?).is_err() {
                    return Err(Problem::TooLarge);
                }
            }
            notes::Recall::Search { scopes, query: value.required(b"query")?.text()? }
        }
        _ => return Err(Problem::Range),
    };
    Ok((by, page))
}

fn scope(value: &Value) -> Result<notes::Scope, Problem> {
    match value.required(b"kind")?.text()?.as_ref() {
        b"deployment" => Ok(notes::Scope::Deployment),
        b"project" => Ok(notes::Scope::Project { project: value.required(b"project")?.small()? }),
        b"goal" => Ok(notes::Scope::Goal {
            project: value.required(b"project")?.small()?,
            goal: value.required(b"goal")?.number()?,
        }),
        b"resources" => {
            let pattern = value.required(b"pattern")?;
            let values = pattern.required(b"segments")?.array()?;
            let Ok(capacity) = u32::try_from(values.len()) else { return Err(Problem::TooLarge) };
            let mut segments = List::with_capacity(capacity);
            for segment in values {
                if segments.push(segment.text()?).is_err() {
                    return Err(Problem::TooLarge);
                }
            }
            let last = match pattern.required(b"terminal")?.text()?.as_ref() {
                b"exact" => notes::Last::Exact(pattern.required(b"last")?.text()?),
                b"open" => notes::Last::Open(pattern.required(b"last")?.text()?),
                _ => return Err(Problem::Range),
            };
            Ok(notes::Scope::Resources {
                project: value.required(b"project")?.small()?,
                connector: value.required(b"connector")?.narrow()?,
                pattern: notes::Pattern { segments: segments.into_boxed(), last },
            })
        }
        _ => Err(Problem::Range),
    }
}

fn numbers(value: &Value) -> Result<List<u64>, Problem> {
    let values = value.array()?;
    let Ok(capacity) = u32::try_from(values.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for value in values {
        let number = value.number()?;
        if number == 0 {
            return Err(Problem::Range);
        }
        if out.push(number).is_err() {
            return Err(Problem::TooLarge);
        }
    }
    Ok(out)
}
