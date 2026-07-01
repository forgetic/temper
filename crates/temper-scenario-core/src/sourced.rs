// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml::Value;

/// A TOML value annotated with the scenario directory that declared it.
#[derive(Debug, Clone)]
pub(crate) struct SourcedValue {
    pub(crate) kind: SourcedValueKind,
    pub(crate) origin_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) enum SourcedValueKind {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Datetime(toml::value::Datetime),
    Array(Vec<SourcedValue>),
    Table(BTreeMap<String, SourcedValue>),
}

impl SourcedValue {
    pub(crate) fn from_value(value: Value, origin_dir: PathBuf) -> Self {
        match value {
            Value::String(value) => Self {
                kind: SourcedValueKind::String(value),
                origin_dir,
            },
            Value::Integer(value) => Self {
                kind: SourcedValueKind::Integer(value),
                origin_dir,
            },
            Value::Float(value) => Self {
                kind: SourcedValueKind::Float(value),
                origin_dir,
            },
            Value::Boolean(value) => Self {
                kind: SourcedValueKind::Boolean(value),
                origin_dir,
            },
            Value::Datetime(value) => Self {
                kind: SourcedValueKind::Datetime(value),
                origin_dir,
            },
            Value::Array(items) => Self {
                kind: SourcedValueKind::Array(
                    items
                        .into_iter()
                        .map(|item| Self::from_value(item, origin_dir.clone()))
                        .collect(),
                ),
                origin_dir,
            },
            Value::Table(table) => Self {
                kind: SourcedValueKind::Table(
                    table
                        .into_iter()
                        .map(|(key, value)| (key, Self::from_value(value, origin_dir.clone())))
                        .collect(),
                ),
                origin_dir,
            },
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        match &self.kind {
            SourcedValueKind::String(value) => Value::String(value.clone()),
            SourcedValueKind::Integer(value) => Value::Integer(*value),
            SourcedValueKind::Float(value) => Value::Float(*value),
            SourcedValueKind::Boolean(value) => Value::Boolean(*value),
            SourcedValueKind::Datetime(value) => Value::Datetime(*value),
            SourcedValueKind::Array(items) => {
                Value::Array(items.iter().map(SourcedValue::to_value).collect())
            }
            SourcedValueKind::Table(table) => Value::Table(
                table
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_value()))
                    .collect(),
            ),
        }
    }

    pub(crate) fn origin_dir(&self) -> &Path {
        &self.origin_dir
    }
}

pub(crate) fn overlay(parent: SourcedValue, child: SourcedValue) -> SourcedValue {
    let child_origin_dir = child.origin_dir.clone();
    match (parent.kind, child.kind) {
        (SourcedValueKind::Table(parent), SourcedValueKind::Table(child_table)) => {
            let mut merged = parent;
            for (key, child_value) in child_table {
                let value = match merged.remove(&key) {
                    Some(parent_value) => overlay(parent_value, child_value),
                    None => child_value,
                };
                merged.insert(key, value);
            }
            SourcedValue {
                kind: SourcedValueKind::Table(merged),
                origin_dir: child_origin_dir,
            }
        }
        (_, child_kind) => SourcedValue {
            kind: child_kind,
            origin_dir: child_origin_dir,
        },
    }
}
