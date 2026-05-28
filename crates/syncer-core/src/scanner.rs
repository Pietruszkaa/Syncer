use camino::{Utf8Path, Utf8PathBuf};
use time::OffsetDateTime;
use tokio::fs;

use crate::error::{CoreError, CoreResult};
use crate::folder_store::SYNCER_DIR_NAME;
use crate::ids::DeviceId;
use crate::manifest::{ContentHash, FileEntry, FileKind, FileVersion, RelativePath};
use crate::state_db::StateDatabase;

#[derive(Clone, Debug)]
pub struct LinuxFolderScanner {
    root: Utf8PathBuf,
    device_id: DeviceId,
}

impl LinuxFolderScanner {
    /// Creates a scanner for a native Linux folder.
    ///
    /// # Errors
    ///
    /// Returns an error when `root` is not valid UTF-8.
    pub fn from_std_path(
        root: impl AsRef<std::path::Path>,
        device_id: DeviceId,
    ) -> CoreResult<Self> {
        let root = Utf8PathBuf::from_path_buf(root.as_ref().to_path_buf()).map_err(|path| {
            CoreError::Filesystem {
                path: path.display().to_string(),
                message: "path is not valid UTF-8".to_owned(),
            }
        })?;

        Ok(Self { root, device_id })
    }

    #[must_use]
    pub fn new(root: Utf8PathBuf, device_id: DeviceId) -> Self {
        Self { root, device_id }
    }

    /// Scans the folder, updates the state database, and returns a summary.
    ///
    /// # Errors
    ///
    /// Returns an error when the folder cannot be walked, files cannot be read,
    /// or the state database rejects an update.
    pub async fn scan_into(&self, database: &StateDatabase) -> CoreResult<ScanSummary> {
        database.mark_scan_started().await?;
        let mut summary = ScanSummary::default();
        let mut stack = vec![self.root.clone()];

        while let Some(directory) = stack.pop() {
            let mut entries =
                fs::read_dir(&directory)
                    .await
                    .map_err(|error| CoreError::Filesystem {
                        path: directory.to_string(),
                        message: error.to_string(),
                    })?;

            while let Some(entry) =
                entries
                    .next_entry()
                    .await
                    .map_err(|error| CoreError::Filesystem {
                        path: directory.to_string(),
                        message: error.to_string(),
                    })?
            {
                let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
                    CoreError::Filesystem {
                        path: path.display().to_string(),
                        message: "path is not valid UTF-8".to_owned(),
                    }
                })?;

                let relative_raw = self.raw_relative_path(&path)?;
                if relative_raw
                    .components()
                    .next()
                    .is_some_and(|component| component.as_str() == SYNCER_DIR_NAME)
                {
                    continue;
                }
                let relative = RelativePath::parse(relative_raw.as_str())?;

                let metadata = entry
                    .metadata()
                    .await
                    .map_err(|error| CoreError::Filesystem {
                        path: path.to_string(),
                        message: error.to_string(),
                    })?;

                if metadata.is_dir() {
                    stack.push(path);
                    summary.directories_seen = summary.directories_seen.saturating_add(1);
                    continue;
                }

                if !metadata.is_file() {
                    summary.unsupported_entries = summary.unsupported_entries.saturating_add(1);
                    continue;
                }

                let bytes = fs::read(&path)
                    .await
                    .map_err(|error| CoreError::Filesystem {
                        path: path.to_string(),
                        message: error.to_string(),
                    })?;
                let size_bytes =
                    u64::try_from(bytes.len()).map_err(|_| CoreError::FileSizeOverflow)?;
                let modified_at = metadata
                    .modified()
                    .ok()
                    .and_then(|modified| {
                        modified
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .and_then(|duration| i64::try_from(duration.as_secs()).ok())
                    })
                    .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
                    .unwrap_or_else(OffsetDateTime::now_utc);
                let entry = FileEntry {
                    path: relative,
                    kind: FileKind::File,
                    size_bytes,
                    modified_at,
                    content_hash: Some(ContentHash::from_bytes(&bytes)),
                    block_hashes: Vec::new(),
                    version: FileVersion {
                        generation: 1,
                        device_id: self.device_id,
                    },
                };

                database.upsert_file(&entry).await?;
                summary.files_seen = summary.files_seen.saturating_add(1);
                summary.bytes_seen = summary.bytes_seen.saturating_add(size_bytes);
            }
        }

        summary.local_missing = database.list_missing_after_scan().await?.len() as u64;
        Ok(summary)
    }

    fn raw_relative_path<'path>(&self, path: &'path Utf8Path) -> CoreResult<&'path Utf8Path> {
        path.strip_prefix(&self.root)
            .map_err(|error| CoreError::Filesystem {
                path: path.to_string(),
                message: error.to_string(),
            })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ScanSummary {
    pub files_seen: u64,
    pub directories_seen: u64,
    pub bytes_seen: u64,
    pub unsupported_entries: u64,
    pub local_missing: u64,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{DeviceId, LinuxFolderScanner, StateDatabase};

    #[tokio::test]
    async fn scanner_ignores_syncer_metadata() -> Result<(), crate::CoreError> {
        let root = std::env::temp_dir().join(format!("syncer-scan-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(root.join(".syncer"))?;
        fs::write(root.join("file.txt"), b"hello")?;
        fs::write(root.join(".syncer/state.db"), b"metadata")?;

        let database = StateDatabase::in_memory().await?;
        let scanner = LinuxFolderScanner::from_std_path(&root, DeviceId::new())?;
        let summary = scanner.scan_into(&database).await?;
        let files = database.list_files().await?;

        assert_eq!(summary.files_seen, 1);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_path().as_str(), "file.txt");
        fs::remove_dir_all(root).ok();
        Ok(())
    }
}
