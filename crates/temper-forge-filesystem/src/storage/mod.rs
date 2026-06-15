//! Filesystem storage layer for the backend.
//!
//! Split by responsibility: [`layout`] manages the on-disk directory layout
//! and atomic JSON read/write, [`paths`] derives the path of every record
//! file, [`records`] reads and writes typed record collections, and
//! [`lookups`] resolves records by id or path across the store.
//!
//! Each submodule adds inherent methods to [`crate::FilesystemForge`]; there is
//! no separate public surface beyond those methods.

mod layout;
mod lookups;
mod paths;
mod provisioning;
mod records;

pub(crate) use provisioning::{UserRecord, WebhookRecord};
