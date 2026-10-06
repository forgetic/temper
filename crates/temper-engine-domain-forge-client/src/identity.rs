//! Deployment keys and authenticated forge writers (domain/forge.md, section 6).
//!
//! The client's configuration names writers and key framing. This module
//! keeps no runtime state and never decides a task's authority. Validation
//! rejects unbounded or ambiguous keys before the client accepts it.
use crate::{Domain, Limits, api};
use alloc::boxed::Box;

/// Root-supplied authenticated account for a forge. Repository identifiers
/// share that forge's connection and writer account.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Writer {
    pub forge: u16,
    pub author: u64,
}

/// Stable across generations. The root derives `namespace` from its durable
/// deployment ID; the aggregate namespace/table allocation is <= `op_bytes`.
/// A decoded key is framed as version 1, a big-endian u16 namespace length,
/// that exact namespace, then a nonempty opaque effect identifier. Another
/// party copying the bytes does not acquire writer provenance.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    pub namespace: Box<[u8]>,
    pub writers: Box<[Writer]>,
}

pub(crate) fn valid(config: &Config, l: &Limits) -> bool {
    if config.namespace.is_empty()
        || config.namespace.len() > usize::from(u16::MAX)
        || config.writers.is_empty()
        || config.writers.len() > usize::try_from(l.repositories).expect("u32 fits usize")
    {
        return false;
    }
    let bytes = config.writers.len().checked_mul(size_of::<Writer>());
    let bytes = match bytes {
        Some(bytes) => bytes.checked_add(config.namespace.len()),
        None => None,
    };
    match bytes {
        Some(bytes) if bytes <= usize::try_from(l.op_bytes).expect("u32 fits usize") => {}
        Some(_) | None => return false,
    }
    for (index, writer) in config.writers.iter().enumerate() {
        if writer.author == 0 {
            return false;
        }
        for previous in config.writers.get(..index).expect("enumerated writer index") {
            if previous.forge == writer.forge {
                return false;
            }
        }
    }
    true
}

pub(crate) fn writer(d: &Domain, repo: api::Repository, author: u64) -> bool {
    match &d.config {
        Some(config) => {
            for writer in &config.writers {
                if writer.forge == repo.forge && writer.author == author {
                    return true;
                }
            }
            false
        }
        None => false,
    }
}

pub(crate) fn historical(d: &Domain, key: &[u8]) -> bool {
    let Some(config) = &d.config else {
        return false;
    };
    let Some(version) = key.first() else {
        return false;
    };
    let Some(high) = key.get(1) else {
        return false;
    };
    let Some(low) = key.get(2) else {
        return false;
    };
    if *version != 1 {
        return false;
    }
    let length = usize::from(u16::from_be_bytes([*high, *low]));
    length == config.namespace.len()
        && key.len() > 3 + length
        && key.get(3..3 + length) == Some(config.namespace.as_ref())
}

pub(crate) fn connected(d: &Domain, repo: api::Repository) -> bool {
    match &d.config {
        Some(config) => {
            for writer in &config.writers {
                if writer.forge == repo.forge {
                    return true;
                }
            }
            false
        }
        None => false,
    }
}
