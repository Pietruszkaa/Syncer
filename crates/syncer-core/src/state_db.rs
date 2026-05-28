use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::manifest::{ContentHash, FileEntry, FileKind, RelativePath};

const SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Debug)]
pub struct StateDatabase {
    pool: SqlitePool,
}

impl StateDatabase {
    /// Opens or creates a folder state database and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot open the database or apply schema.
    pub async fn open(path: &Utf8Path) -> CoreResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let database = Self { pool };
        database.migrate().await?;
        Ok(database)
    }

    /// Opens an in-memory database for tests.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot create the in-memory database.
    pub async fn in_memory() -> CoreResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let database = Self { pool };
        database.migrate().await?;
        Ok(database)
    }

    /// Inserts or updates a scanned local file entry.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the write.
    pub async fn upsert_file(&self, entry: &FileEntry) -> CoreResult<()> {
        let kind = match entry.kind {
            FileKind::File => "file",
            FileKind::Directory => "directory",
        };
        let content_hash = entry.content_hash.as_ref().map(ContentHash::as_hex);
        let block_hashes = serde_json::to_string(&entry.block_hashes)?;
        let modified_at = entry.modified_at.unix_timestamp();
        let size_bytes =
            i64::try_from(entry.size_bytes).map_err(|_| CoreError::FileSizeOverflow)?;
        let generation = i64::try_from(entry.version.generation)
            .map_err(|_| CoreError::StateDatabase("generation exceeded i64".to_owned()))?;

        sqlx::query(
            r"
            INSERT INTO files (
              path, kind, size_bytes, modified_at_unix, content_hash,
              block_hashes_json, generation, device_id, sync_state
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'local_available')
            ON CONFLICT(path) DO UPDATE SET
              kind = excluded.kind,
              size_bytes = excluded.size_bytes,
              modified_at_unix = excluded.modified_at_unix,
              content_hash = excluded.content_hash,
              block_hashes_json = excluded.block_hashes_json,
              generation = excluded.generation,
              device_id = excluded.device_id,
              scan_missing = 0,
              sync_state = 'local_available',
              updated_at_unix = unixepoch()
            ",
        )
        .bind(entry.path.as_path().as_str())
        .bind(kind)
        .bind(size_bytes)
        .bind(modified_at)
        .bind(content_hash)
        .bind(block_hashes)
        .bind(generation)
        .bind(entry.version.device_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Marks known local files as temporarily missing before a scan.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the update.
    pub async fn mark_scan_started(&self) -> CoreResult<()> {
        sqlx::query("UPDATE files SET scan_missing = 1 WHERE sync_state != 'skipped_folder_limit'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Converts files not seen during a scan into unsynchronized local-missing state.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the update.
    pub async fn finalize_scan(&self) -> CoreResult<()> {
        sqlx::query(
            r"
            UPDATE files
            SET sync_state = 'unsynchronized_local_missing',
                updated_at_unix = unixepoch()
            WHERE scan_missing = 1
            ",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists files deleted locally outside Syncer and still known to the folder state.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn list_missing_after_scan(&self) -> CoreResult<Vec<FileRecord>> {
        let rows = sqlx::query(
            r"
            SELECT path, kind, size_bytes, modified_at_unix, content_hash,
                   generation, device_id, sync_state
            FROM files
            WHERE sync_state = 'unsynchronized_local_missing'
            ORDER BY path
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_file_record).collect()
    }

    /// Lists all indexed files.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn list_files(&self) -> CoreResult<Vec<FileRecord>> {
        let rows = sqlx::query(
            r"
            SELECT path, kind, size_bytes, modified_at_unix, content_hash,
                   generation, device_id, sync_state
            FROM files
            ORDER BY path
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_file_record).collect()
    }

    /// Records a remote file skipped because the folder size limit would be exceeded.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the write.
    pub async fn record_skipped_folder_limit(
        &self,
        transfer: &crate::PendingTransfer,
    ) -> CoreResult<()> {
        let size_bytes =
            i64::try_from(transfer.size_bytes).map_err(|_| CoreError::FileSizeOverflow)?;
        sqlx::query(
            r"
            INSERT INTO skipped_files (path, reason, size_bytes, queued_at_unix)
            VALUES (?1, 'folder_limit', ?2, ?3)
            ON CONFLICT(path) DO UPDATE SET
              reason = excluded.reason,
              size_bytes = excluded.size_bytes,
              queued_at_unix = excluded.queued_at_unix,
              updated_at_unix = unixepoch()
            ",
        )
        .bind(transfer.path.as_path().as_str())
        .bind(size_bytes)
        .bind(transfer.queued_at.unix_timestamp())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists files skipped by policy.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn list_skipped_files(&self) -> CoreResult<Vec<SkippedFileRecord>> {
        let rows = sqlx::query(
            r"
            SELECT path, reason, size_bytes, queued_at_unix
            FROM skipped_files
            ORDER BY queued_at_unix, path
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_skipped_file_record).collect()
    }

    /// Returns aggregate folder state for UI and CLI status views.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn folder_status(&self) -> CoreResult<FolderStatus> {
        let files = self.list_files().await?;
        let skipped = self.list_skipped_files().await?;
        let mut status = FolderStatus {
            indexed_files: files.len() as u64,
            indexed_bytes: files.iter().map(|file| file.size_bytes).sum(),
            skipped_folder_limit: skipped.len() as u64,
            ..FolderStatus::default()
        };

        for file in files {
            match file.sync_state {
                FileSyncState::LocalAvailable => {
                    status.local_available = status.local_available.saturating_add(1);
                }
                FileSyncState::UnsynchronizedLocalMissing => {
                    status.unsynchronized_local_missing =
                        status.unsynchronized_local_missing.saturating_add(1);
                }
                FileSyncState::PendingUpload => {
                    status.pending_upload = status.pending_upload.saturating_add(1);
                }
                FileSyncState::PendingDownload => {
                    status.pending_download = status.pending_download.saturating_add(1);
                }
                FileSyncState::SkippedFolderLimit => {
                    status.skipped_folder_limit = status.skipped_folder_limit.saturating_add(1);
                }
            }
        }

        Ok(status)
    }

    async fn migrate(&self) -> CoreResult<()> {
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&self.pool)
            .await?;
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS metadata (
              key TEXT PRIMARY KEY NOT NULL,
              value TEXT NOT NULL
            )
            ",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS files (
              path TEXT PRIMARY KEY NOT NULL,
              kind TEXT NOT NULL CHECK (kind IN ('file', 'directory')),
              size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
              modified_at_unix INTEGER NOT NULL,
              content_hash TEXT,
              block_hashes_json TEXT NOT NULL,
              generation INTEGER NOT NULL CHECK (generation >= 0),
              device_id TEXT NOT NULL,
              scan_missing INTEGER NOT NULL DEFAULT 0 CHECK (scan_missing IN (0, 1)),
              sync_state TEXT NOT NULL DEFAULT 'local_available' CHECK (
                sync_state IN (
                  'local_available',
                  'unsynchronized_local_missing',
                  'pending_upload',
                  'pending_download',
                  'skipped_folder_limit'
                )
              ),
              updated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
            )
            ",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS skipped_files (
              path TEXT PRIMARY KEY NOT NULL,
              reason TEXT NOT NULL CHECK (reason IN ('folder_limit')),
              size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
              queued_at_unix INTEGER NOT NULL,
              updated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
            )
            ",
        )
        .execute(&self.pool)
        .await?;
        ensure_column(
            &self.pool,
            "files",
            "sync_state",
            "TEXT NOT NULL DEFAULT 'local_available'",
        )
        .await?;
        sqlx::query(
            r"
            INSERT INTO metadata (key, value)
            VALUES ('schema_version', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
        )
        .bind(SCHEMA_VERSION.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileRecord {
    pub path: RelativePath,
    pub kind: FileKind,
    pub size_bytes: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub modified_at: OffsetDateTime,
    pub content_hash: Option<String>,
    pub generation: u64,
    pub device_id: String,
    pub sync_state: FileSyncState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSyncState {
    LocalAvailable,
    UnsynchronizedLocalMissing,
    PendingUpload,
    PendingDownload,
    SkippedFolderLimit,
}

impl FileSyncState {
    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "local_available" => Ok(Self::LocalAvailable),
            "unsynchronized_local_missing" => Ok(Self::UnsynchronizedLocalMissing),
            "pending_upload" => Ok(Self::PendingUpload),
            "pending_download" => Ok(Self::PendingDownload),
            "skipped_folder_limit" => Ok(Self::SkippedFolderLimit),
            other => Err(CoreError::StateDatabase(format!(
                "unknown file sync state {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkippedFileRecord {
    pub path: RelativePath,
    pub reason: SkippedReason,
    pub size_bytes: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub queued_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkippedReason {
    FolderLimit,
}

impl SkippedReason {
    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "folder_limit" => Ok(Self::FolderLimit),
            other => Err(CoreError::StateDatabase(format!(
                "unknown skipped reason {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderStatus {
    pub indexed_files: u64,
    pub indexed_bytes: u64,
    pub local_available: u64,
    pub unsynchronized_local_missing: u64,
    pub pending_upload: u64,
    pub pending_download: u64,
    pub skipped_folder_limit: u64,
}

fn row_to_file_record(row: &sqlx::sqlite::SqliteRow) -> CoreResult<FileRecord> {
    let path: String = row.try_get("path")?;
    let kind: String = row.try_get("kind")?;
    let size_bytes: i64 = row.try_get("size_bytes")?;
    let modified_at_unix: i64 = row.try_get("modified_at_unix")?;
    let content_hash: Option<String> = row.try_get("content_hash")?;
    let generation: i64 = row.try_get("generation")?;
    let device_id: String = row.try_get("device_id")?;
    let sync_state: String = row.try_get("sync_state")?;

    let kind = match kind.as_str() {
        "file" => FileKind::File,
        "directory" => FileKind::Directory,
        other => {
            return Err(CoreError::StateDatabase(format!(
                "unknown file kind {other}"
            )));
        }
    };

    Ok(FileRecord {
        path: RelativePath::parse(path)?,
        kind,
        size_bytes: u64::try_from(size_bytes).map_err(|_| CoreError::FileSizeOverflow)?,
        modified_at: OffsetDateTime::from_unix_timestamp(modified_at_unix)
            .map_err(|error| CoreError::StateDatabase(error.to_string()))?,
        content_hash,
        generation: u64::try_from(generation)
            .map_err(|_| CoreError::StateDatabase("negative generation".to_owned()))?,
        device_id,
        sync_state: FileSyncState::parse(&sync_state)?,
    })
}

fn row_to_skipped_file_record(row: &sqlx::sqlite::SqliteRow) -> CoreResult<SkippedFileRecord> {
    let path: String = row.try_get("path")?;
    let reason: String = row.try_get("reason")?;
    let size_bytes: i64 = row.try_get("size_bytes")?;
    let queued_at_unix: i64 = row.try_get("queued_at_unix")?;

    Ok(SkippedFileRecord {
        path: RelativePath::parse(path)?,
        reason: SkippedReason::parse(&reason)?,
        size_bytes: u64::try_from(size_bytes).map_err(|_| CoreError::FileSizeOverflow)?,
        queued_at: OffsetDateTime::from_unix_timestamp(queued_at_unix)
            .map_err(|error| CoreError::StateDatabase(error.to_string()))?,
    })
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> CoreResult<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let rows = sqlx::query(&pragma).fetch_all(pool).await?;
    let exists = rows
        .iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .any(|name| name == column);

    if !exists {
        let statement = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
        sqlx::query(&statement).execute(pool).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use crate::{
        ContentHash, DeviceId, FileEntry, FileKind, FileSyncState, FileVersion, PendingTransfer,
        RelativePath, StateDatabase,
    };

    #[tokio::test]
    async fn stores_and_lists_file_entries() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let entry = test_entry("docs/readme.txt")?;

        database.upsert_file(&entry).await?;
        let files = database.list_files().await?;

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_path().as_str(), "docs/readme.txt");
        assert_eq!(files[0].sync_state, FileSyncState::LocalAvailable);
        Ok(())
    }

    #[tokio::test]
    async fn marks_missing_files_as_unsynchronized() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        database.upsert_file(&test_entry("gone.txt")?).await?;
        database.mark_scan_started().await?;
        database.finalize_scan().await?;

        let missing = database.list_missing_after_scan().await?;
        let status = database.folder_status().await?;

        assert_eq!(missing.len(), 1);
        assert_eq!(
            missing[0].sync_state,
            FileSyncState::UnsynchronizedLocalMissing
        );
        assert_eq!(status.unsynchronized_local_missing, 1);
        Ok(())
    }

    #[tokio::test]
    async fn records_skipped_folder_limit_files() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let transfer = PendingTransfer {
            operation_id: crate::OperationId::new(),
            path: RelativePath::parse("large.iso")?,
            size_bytes: 1024,
            queued_at: datetime!(2026-05-28 10:00 UTC),
        };

        database.record_skipped_folder_limit(&transfer).await?;
        let skipped = database.list_skipped_files().await?;
        let status = database.folder_status().await?;

        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].path.as_path().as_str(), "large.iso");
        assert_eq!(status.skipped_folder_limit, 1);
        Ok(())
    }

    fn test_entry(path: &str) -> Result<FileEntry, crate::CoreError> {
        Ok(FileEntry {
            path: RelativePath::parse(path)?,
            kind: FileKind::File,
            size_bytes: 5,
            modified_at: datetime!(2026-05-28 10:00 UTC),
            content_hash: Some(ContentHash::from_bytes(b"hello")),
            block_hashes: Vec::new(),
            version: FileVersion {
                generation: 1,
                device_id: DeviceId::new(),
            },
        })
    }
}
