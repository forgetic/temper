use crate::FilesystemForge;
use crate::errors::backend_error;
use crate::metadata::{Metadata, default_metadata};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use temper_forge::{ForgeError, ForgeResult, RepositoryId};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Builds a per-process-unique temporary file extension.
///
/// Combining the process id with a monotonic counter keeps two writers — even in
/// different OS processes sharing one store — from colliding on the same temp
/// path before the atomic rename (see ADR 0018). The extension is not `json`, so
/// listing code that filters on the `json` extension ignores it.
fn unique_temp_extension() -> String {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("tmp-{}-{counter}", std::process::id())
}

impl FilesystemForge {
    /// Creates the backend directory layout if needed.
    pub fn ensure_layout(&self) -> ForgeResult<()> {
        fs::create_dir_all(self.root()).map_err(|error| {
            backend_error(
                format!("create storage root {}", self.root().display()),
                error,
            )
        })?;
        fs::create_dir_all(self.repositories_dir()).map_err(|error| {
            backend_error(
                format!(
                    "create repositories directory {}",
                    self.repositories_dir().display()
                ),
                error,
            )
        })?;

        let metadata_path = self.metadata_path();
        if metadata_path.exists() {
            self.read_metadata_file()?;
        } else {
            self.write_metadata(&default_metadata())?;
        }
        Ok(())
    }

    pub(crate) fn read_metadata(&self) -> ForgeResult<Metadata> {
        self.ensure_layout()?;
        self.read_metadata_file()
    }

    pub(crate) fn read_metadata_file(&self) -> ForgeResult<Metadata> {
        let metadata: Metadata = self.read_json(&self.metadata_path())?;
        metadata.validate()?;
        Ok(metadata)
    }

    pub(crate) fn write_metadata(&self, metadata: &Metadata) -> ForgeResult<()> {
        metadata.validate()?;
        self.write_json(&self.metadata_path(), metadata)
    }

    pub(crate) fn read_json<T>(&self, path: &Path) -> ForgeResult<T>
    where
        T: DeserializeOwned,
    {
        let content = fs::read_to_string(path)
            .map_err(|error| backend_error(format!("read {}", path.display()), error))?;
        serde_json::from_str(&content)
            .map_err(|error| backend_error(format!("parse {}", path.display()), error))
    }

    pub(crate) fn write_json<T>(&self, path: &Path, value: &T) -> ForgeResult<()>
    where
        T: Serialize,
    {
        let parent = path.parent().ok_or_else(|| {
            ForgeError::Backend(format!("path {} has no parent directory", path.display()))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| backend_error(format!("create {}", parent.display()), error))?;

        let mut content = serde_json::to_string_pretty(value)
            .map_err(|error| backend_error(format!("serialize {}", path.display()), error))?;
        content.push('\n');

        let temporary_path = path.with_extension(unique_temp_extension());
        fs::write(&temporary_path, content)
            .map_err(|error| backend_error(format!("write {}", temporary_path.display()), error))?;
        fs::rename(&temporary_path, path).map_err(|error| {
            let _ = fs::remove_file(&temporary_path);
            backend_error(
                format!(
                    "replace {} from {}",
                    path.display(),
                    temporary_path.display()
                ),
                error,
            )
        })?;

        Ok(())
    }

    pub(crate) fn next_repository_id(&self, metadata: &mut Metadata) -> ForgeResult<RepositoryId> {
        loop {
            let repository_number = metadata.next_repository_number;
            metadata.next_repository_number = metadata
                .next_repository_number
                .checked_add(1)
                .ok_or_else(|| ForgeError::Backend("repository id counter overflowed".into()))?;

            let id = RepositoryId::new(format!("repo-{repository_number:016}"));
            let path = self.repository_file(&id).ok_or_else(|| {
                ForgeError::Backend(format!("generated invalid repository id {id}"))
            })?;
            if !path.exists() {
                return Ok(id);
            }
        }
    }
}
