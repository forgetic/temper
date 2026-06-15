// SPDX-License-Identifier: MPL-2.0

//! Configuration errors. Every variant names the file and the problem so an
//! operator can fix it without reading source.

use std::path::PathBuf;

use thiserror::Error;

/// What kind of file a [`ConfigError`] is about, for friendly messages.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FileKind {
    /// The non-secret config file.
    Config,
    /// The secrets/credentials file.
    Credentials,
}

impl FileKind {
    /// The lowercase noun used in error messages.
    pub fn noun(self) -> &'static str {
        match self {
            FileKind::Config => "config",
            FileKind::Credentials => "credentials",
        }
    }
}

/// A failure loading, parsing, or resolving configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The file could not be read from disk.
    #[error("reading {kind} file {path}: {source}", kind = kind.noun(), path = path.display())]
    Read {
        kind: FileKind,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file was not valid TOML or had an unknown/mistyped key.
    #[error("parsing {kind} file {path}: {message}", kind = kind.noun(), path = path.display())]
    Parse {
        kind: FileKind,
        path: PathBuf,
        message: String,
    },

    /// `schema_version` was missing, non-integer, or unsupported.
    #[error("{kind} file {path}: {message}", kind = kind.noun(), path = path.display())]
    SchemaVersion {
        kind: FileKind,
        path: PathBuf,
        message: String,
    },

    /// A required value was absent everywhere (file, env, default).
    #[error("{message}")]
    Missing { message: String },

    /// A value was present but malformed (bad address, bad `owner/name`, …).
    #[error("{message}")]
    Invalid { message: String },
}

impl ConfigError {
    /// Builds a [`ConfigError::Missing`].
    pub fn missing(message: impl Into<String>) -> Self {
        ConfigError::Missing {
            message: message.into(),
        }
    }

    /// Builds a [`ConfigError::Invalid`].
    pub fn invalid(message: impl Into<String>) -> Self {
        ConfigError::Invalid {
            message: message.into(),
        }
    }
}
