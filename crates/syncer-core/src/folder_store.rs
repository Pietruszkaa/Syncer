use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::error::{CoreError, CoreResult};
use crate::ids::FolderId;
use crate::model::{FolderConfig, FolderMode, FolderPeer, RetentionPolicy, StorageRoot};

pub const CURRENT_FOLDER_SCHEMA_VERSION: u16 = 1;
pub const SYNCER_DIR_NAME: &str = ".syncer";
pub const FOLDER_CONFIG_FILE_NAME: &str = "folder.json";
pub const STATE_DB_FILE_NAME: &str = "state.db";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderDocument {
    pub schema_version: u16,
    pub folder: FolderConfig,
}

impl FolderDocument {
    #[must_use]
    pub fn new(folder: FolderConfig) -> Self {
        Self {
            schema_version: CURRENT_FOLDER_SCHEMA_VERSION,
            folder,
        }
    }

    /// Validates this document schema and embedded folder configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema version is unsupported.
    pub fn validate(&self) -> CoreResult<()> {
        if self.schema_version != CURRENT_FOLDER_SCHEMA_VERSION {
            return Err(CoreError::UnsupportedFolderSchema {
                found: self.schema_version,
            });
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FolderStore {
    root: Utf8PathBuf,
}

impl FolderStore {
    /// Creates a store for a native folder path.
    ///
    /// # Errors
    ///
    /// Returns an error when `root` is not valid UTF-8.
    pub fn from_std_path(root: impl AsRef<std::path::Path>) -> CoreResult<Self> {
        let root = Utf8PathBuf::from_path_buf(root.as_ref().to_path_buf()).map_err(|path| {
            CoreError::Filesystem {
                path: path.display().to_string(),
                message: "path is not valid UTF-8".to_owned(),
            }
        })?;

        Ok(Self { root })
    }

    #[must_use]
    pub fn new(root: Utf8PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    #[must_use]
    pub fn syncer_dir(&self) -> Utf8PathBuf {
        self.root.join(SYNCER_DIR_NAME)
    }

    #[must_use]
    pub fn folder_json_path(&self) -> Utf8PathBuf {
        self.syncer_dir().join(FOLDER_CONFIG_FILE_NAME)
    }

    #[must_use]
    pub fn state_db_path(&self) -> Utf8PathBuf {
        self.syncer_dir().join(STATE_DB_FILE_NAME)
    }

    /// Creates `.syncer` and writes `folder.json`.
    ///
    /// # Errors
    ///
    /// Returns an error when directories or JSON files cannot be written.
    pub async fn initialize(
        &self,
        display_name: String,
        mode: FolderMode,
        size_limit: crate::FolderSizeLimit,
        peers: Vec<FolderPeer>,
    ) -> CoreResult<FolderDocument> {
        let document = FolderDocument::new(FolderConfig {
            id: FolderId::new(),
            display_name,
            root: StorageRoot::NativePath(self.root.clone()),
            mode,
            interval_seconds: 900,
            size_limit,
            retention: RetentionPolicy::default(),
            peers,
        });

        self.write(&document).await?;
        Ok(document)
    }

    /// Reads and validates `.syncer/folder.json`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, parsed, or validated.
    pub async fn read(&self) -> CoreResult<FolderDocument> {
        let path = self.folder_json_path();
        let bytes = fs::read(&path)
            .await
            .map_err(|error| CoreError::Filesystem {
                path: path.to_string(),
                message: error.to_string(),
            })?;
        let document: FolderDocument = serde_json::from_slice(&bytes)?;
        document.validate()?;
        Ok(document)
    }

    /// Writes `.syncer/folder.json` atomically through a temporary file.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or the file cannot
    /// be serialized, written, flushed, or renamed.
    pub async fn write(&self, document: &FolderDocument) -> CoreResult<()> {
        document.validate()?;
        let syncer_dir = self.syncer_dir();
        fs::create_dir_all(&syncer_dir)
            .await
            .map_err(|error| CoreError::Filesystem {
                path: syncer_dir.to_string(),
                message: error.to_string(),
            })?;

        let target = self.folder_json_path();
        let temporary = syncer_dir.join("folder.json.tmp");
        let bytes = serde_json::to_vec_pretty(document)?;
        fs::write(&temporary, bytes)
            .await
            .map_err(|error| CoreError::Filesystem {
                path: temporary.to_string(),
                message: error.to_string(),
            })?;
        fs::rename(&temporary, &target)
            .await
            .map_err(|error| CoreError::Filesystem {
                path: target.to_string(),
                message: error.to_string(),
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{FolderMode, FolderSizeLimit};

    use super::FolderStore;

    #[tokio::test]
    async fn writes_and_reads_folder_document() -> Result<(), crate::CoreError> {
        let root =
            std::env::temp_dir().join(format!("syncer-folder-store-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).map_err(|error| crate::CoreError::Filesystem {
            path: root.display().to_string(),
            message: error.to_string(),
        })?;

        let store = FolderStore::from_std_path(&root)?;
        let size_limit = FolderSizeLimit::new(1024, 80)?;
        let written = store
            .initialize(
                "Photos".to_owned(),
                FolderMode::Bidirectional,
                size_limit,
                Vec::new(),
            )
            .await?;
        let read = store.read().await?;

        assert_eq!(read, written);
        fs::remove_dir_all(root).ok();
        Ok(())
    }
}
