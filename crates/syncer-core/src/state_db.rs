use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::ids::{DeviceId, FolderId};
use crate::manifest::{ContentHash, FileEntry, FileKind, RelativePath};
use crate::{SyncAction, SyncPlan};

const SCHEMA_VERSION: i64 = 4;

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

    /// Exports the indexed folder manifest for peer comparison.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn export_manifest(
        &self,
        folder_id: FolderId,
        device_id: DeviceId,
    ) -> CoreResult<FolderManifest> {
        Ok(FolderManifest {
            folder_id,
            device_id,
            generated_at: OffsetDateTime::now_utc(),
            files: self.list_files().await?,
        })
    }

    /// Replaces queued operations for a remote peer with operations from a sync plan.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the transaction.
    pub async fn enqueue_sync_plan(&self, plan: &SyncPlan) -> CoreResult<QueueStatus> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            r"
            DELETE FROM sync_operations
            WHERE remote_device_id = ?1
              AND status IN ('pending', 'blocked')
            ",
        )
        .bind(&plan.remote_device_id)
        .execute(&mut *transaction)
        .await?;

        for entry in &plan.entries {
            let Some((kind, status)) = operation_for_action(entry.action) else {
                continue;
            };
            let local_size_bytes = optional_i64(entry.local_size_bytes)?;
            let remote_size_bytes = optional_i64(entry.remote_size_bytes)?;

            sqlx::query(
                r"
                INSERT INTO sync_operations (
                  id, path, kind, status, reason, local_size_bytes,
                  remote_size_bytes, local_content_hash, remote_content_hash,
                  remote_device_id
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(&entry.path)
            .bind(kind.as_str())
            .bind(status.as_str())
            .bind(&entry.reason)
            .bind(local_size_bytes)
            .bind(remote_size_bytes)
            .bind(&entry.local_content_hash)
            .bind(&entry.remote_content_hash)
            .bind(&plan.remote_device_id)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        self.queue_status().await
    }

    /// Lists queued sync operations.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn list_sync_operations(&self) -> CoreResult<Vec<SyncOperationRecord>> {
        let rows = sqlx::query(
            r"
            SELECT id, path, kind, status, reason, local_size_bytes, remote_size_bytes,
                   local_content_hash, remote_content_hash, remote_device_id,
                   last_error, created_at_unix, updated_at_unix
            FROM sync_operations
            ORDER BY created_at_unix, path
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_sync_operation).collect()
    }

    /// Lists pending download operations for one remote peer.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn list_pending_download_operations(
        &self,
        remote_device_id: &str,
        limit: u64,
    ) -> CoreResult<Vec<SyncOperationRecord>> {
        let limit = i64::try_from(limit).map_err(|_| CoreError::FileSizeOverflow)?;
        let rows = sqlx::query(
            r"
            SELECT id, path, kind, status, reason, local_size_bytes, remote_size_bytes,
                   local_content_hash, remote_content_hash, remote_device_id,
                   last_error, created_at_unix, updated_at_unix
            FROM sync_operations
            WHERE remote_device_id = ?1
              AND kind = 'download_from_remote'
              AND status = 'pending'
            ORDER BY created_at_unix, path
            LIMIT ?2
            ",
        )
        .bind(remote_device_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_sync_operation).collect()
    }

    /// Marks a queued operation as successfully completed.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the update.
    pub async fn mark_sync_operation_done(&self, operation_id: &str) -> CoreResult<()> {
        self.update_sync_operation_status(operation_id, SyncOperationStatus::Done, None)
            .await
    }

    /// Marks a queued operation as failed and stores the public failure reason.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the update.
    pub async fn mark_sync_operation_failed(
        &self,
        operation_id: &str,
        error: &str,
    ) -> CoreResult<()> {
        self.update_sync_operation_status(operation_id, SyncOperationStatus::Failed, Some(error))
            .await
    }

    async fn update_sync_operation_status(
        &self,
        operation_id: &str,
        status: SyncOperationStatus,
        error: Option<&str>,
    ) -> CoreResult<()> {
        sqlx::query(
            r"
            UPDATE sync_operations
            SET status = ?2,
                last_error = ?3,
                updated_at_unix = unixepoch()
            WHERE id = ?1
            ",
        )
        .bind(operation_id)
        .bind(status.as_str())
        .bind(error)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Returns aggregate queue state.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn queue_status(&self) -> CoreResult<QueueStatus> {
        let operations = self.list_sync_operations().await?;
        let mut status = QueueStatus {
            total: operations.len() as u64,
            ..QueueStatus::default()
        };

        for operation in operations {
            match operation.status {
                SyncOperationStatus::Pending => {
                    status.pending = status.pending.saturating_add(1);
                }
                SyncOperationStatus::Blocked => {
                    status.blocked = status.blocked.saturating_add(1);
                }
                SyncOperationStatus::Done => {
                    status.done = status.done.saturating_add(1);
                }
                SyncOperationStatus::Failed => {
                    status.failed = status.failed.saturating_add(1);
                }
            }

            match operation.kind {
                SyncOperationKind::UploadToRemote => {
                    status.upload_to_remote = status.upload_to_remote.saturating_add(1);
                }
                SyncOperationKind::DownloadFromRemote => {
                    status.download_from_remote = status.download_from_remote.saturating_add(1);
                }
                SyncOperationKind::ResolveConflict => {
                    status.resolve_conflict = status.resolve_conflict.saturating_add(1);
                }
                SyncOperationKind::RepairLocalMissing => {
                    status.repair_local_missing = status.repair_local_missing.saturating_add(1);
                }
                SyncOperationKind::RepairRemoteMissing => {
                    status.repair_remote_missing = status.repair_remote_missing.saturating_add(1);
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
        self.create_metadata_table().await?;
        self.create_files_table().await?;
        self.create_skipped_files_table().await?;
        self.create_sync_operations_table().await?;
        ensure_column(
            &self.pool,
            "files",
            "sync_state",
            "TEXT NOT NULL DEFAULT 'local_available'",
        )
        .await?;
        ensure_column(&self.pool, "sync_operations", "local_content_hash", "TEXT").await?;
        ensure_column(&self.pool, "sync_operations", "remote_content_hash", "TEXT").await?;
        ensure_column(&self.pool, "sync_operations", "last_error", "TEXT").await?;
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

    async fn create_metadata_table(&self) -> CoreResult<()> {
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
        Ok(())
    }

    async fn create_files_table(&self) -> CoreResult<()> {
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
        Ok(())
    }

    async fn create_skipped_files_table(&self) -> CoreResult<()> {
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
        Ok(())
    }

    async fn create_sync_operations_table(&self) -> CoreResult<()> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS sync_operations (
              id TEXT PRIMARY KEY NOT NULL,
              path TEXT NOT NULL,
              kind TEXT NOT NULL CHECK (
                kind IN (
                  'upload_to_remote',
                  'download_from_remote',
                  'resolve_conflict',
                  'repair_local_missing',
                  'repair_remote_missing'
                )
              ),
              status TEXT NOT NULL CHECK (
                status IN ('pending', 'blocked', 'done', 'failed')
              ),
              reason TEXT NOT NULL,
              local_size_bytes INTEGER CHECK (local_size_bytes IS NULL OR local_size_bytes >= 0),
              remote_size_bytes INTEGER CHECK (remote_size_bytes IS NULL OR remote_size_bytes >= 0),
              local_content_hash TEXT,
              remote_content_hash TEXT,
              remote_device_id TEXT NOT NULL,
              last_error TEXT,
              created_at_unix INTEGER NOT NULL DEFAULT (unixepoch()),
              updated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
            )
            ",
        )
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderManifest {
    pub folder_id: FolderId,
    pub device_id: DeviceId,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    pub files: Vec<FileRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SyncOperationRecord {
    pub id: String,
    pub path: RelativePath,
    pub kind: SyncOperationKind,
    pub status: SyncOperationStatus,
    pub reason: String,
    pub local_size_bytes: Option<u64>,
    pub remote_size_bytes: Option<u64>,
    pub local_content_hash: Option<String>,
    pub remote_content_hash: Option<String>,
    pub remote_device_id: String,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperationKind {
    UploadToRemote,
    DownloadFromRemote,
    ResolveConflict,
    RepairLocalMissing,
    RepairRemoteMissing,
}

impl SyncOperationKind {
    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "upload_to_remote" => Ok(Self::UploadToRemote),
            "download_from_remote" => Ok(Self::DownloadFromRemote),
            "resolve_conflict" => Ok(Self::ResolveConflict),
            "repair_local_missing" => Ok(Self::RepairLocalMissing),
            "repair_remote_missing" => Ok(Self::RepairRemoteMissing),
            other => Err(CoreError::StateDatabase(format!(
                "unknown sync operation kind {other}"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::UploadToRemote => "upload_to_remote",
            Self::DownloadFromRemote => "download_from_remote",
            Self::ResolveConflict => "resolve_conflict",
            Self::RepairLocalMissing => "repair_local_missing",
            Self::RepairRemoteMissing => "repair_remote_missing",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperationStatus {
    Pending,
    Blocked,
    Done,
    Failed,
}

impl SyncOperationStatus {
    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "blocked" => Ok(Self::Blocked),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            other => Err(CoreError::StateDatabase(format!(
                "unknown sync operation status {other}"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueueStatus {
    pub total: u64,
    pub pending: u64,
    pub blocked: u64,
    pub done: u64,
    pub failed: u64,
    pub upload_to_remote: u64,
    pub download_from_remote: u64,
    pub resolve_conflict: u64,
    pub repair_local_missing: u64,
    pub repair_remote_missing: u64,
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

fn row_to_sync_operation(row: &sqlx::sqlite::SqliteRow) -> CoreResult<SyncOperationRecord> {
    let id: String = row.try_get("id")?;
    let path: String = row.try_get("path")?;
    let kind: String = row.try_get("kind")?;
    let status: String = row.try_get("status")?;
    let reason: String = row.try_get("reason")?;
    let local_size_bytes: Option<i64> = row.try_get("local_size_bytes")?;
    let remote_size_bytes: Option<i64> = row.try_get("remote_size_bytes")?;
    let local_content_hash: Option<String> = row.try_get("local_content_hash")?;
    let remote_content_hash: Option<String> = row.try_get("remote_content_hash")?;
    let remote_device_id: String = row.try_get("remote_device_id")?;
    let last_error: Option<String> = row.try_get("last_error")?;
    let created_at_unix: i64 = row.try_get("created_at_unix")?;
    let updated_at_unix: i64 = row.try_get("updated_at_unix")?;

    Ok(SyncOperationRecord {
        id,
        path: RelativePath::parse(path)?,
        kind: SyncOperationKind::parse(&kind)?,
        status: SyncOperationStatus::parse(&status)?,
        reason,
        local_size_bytes: optional_u64(local_size_bytes)?,
        remote_size_bytes: optional_u64(remote_size_bytes)?,
        local_content_hash,
        remote_content_hash,
        remote_device_id,
        last_error,
        created_at: unix_timestamp(created_at_unix)?,
        updated_at: unix_timestamp(updated_at_unix)?,
    })
}

fn operation_for_action(action: SyncAction) -> Option<(SyncOperationKind, SyncOperationStatus)> {
    match action {
        SyncAction::InSync => None,
        SyncAction::UploadToRemote => Some((
            SyncOperationKind::UploadToRemote,
            SyncOperationStatus::Pending,
        )),
        SyncAction::DownloadFromRemote => Some((
            SyncOperationKind::DownloadFromRemote,
            SyncOperationStatus::Pending,
        )),
        SyncAction::Conflict => Some((
            SyncOperationKind::ResolveConflict,
            SyncOperationStatus::Blocked,
        )),
        SyncAction::LocalMissingUnsynchronized => Some((
            SyncOperationKind::RepairLocalMissing,
            SyncOperationStatus::Blocked,
        )),
        SyncAction::RemoteMissingUnsynchronized => Some((
            SyncOperationKind::RepairRemoteMissing,
            SyncOperationStatus::Blocked,
        )),
    }
}

fn optional_i64(value: Option<u64>) -> CoreResult<Option<i64>> {
    value
        .map(|value| i64::try_from(value).map_err(|_| CoreError::FileSizeOverflow))
        .transpose()
}

fn optional_u64(value: Option<i64>) -> CoreResult<Option<u64>> {
    value
        .map(|value| u64::try_from(value).map_err(|_| CoreError::FileSizeOverflow))
        .transpose()
}

fn unix_timestamp(timestamp: i64) -> CoreResult<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .map_err(|error| CoreError::StateDatabase(error.to_string()))
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
        RelativePath, StateDatabase, SyncAction, SyncOperationKind, SyncOperationStatus, SyncPlan,
        SyncPlanEntry, SyncPlanSummary,
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

    #[tokio::test]
    async fn enqueues_actionable_sync_plan_entries() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let plan = SyncPlan {
            local_device_id: DeviceId::new().to_string(),
            remote_device_id: DeviceId::new().to_string(),
            entries: vec![
                plan_entry("same.txt", SyncAction::InSync),
                plan_entry("upload.txt", SyncAction::UploadToRemote),
                plan_entry("download.txt", SyncAction::DownloadFromRemote),
                plan_entry("conflict.txt", SyncAction::Conflict),
            ],
            summary: SyncPlanSummary::default(),
        };

        let status = database.enqueue_sync_plan(&plan).await?;
        let operations = database.list_sync_operations().await?;

        assert_eq!(status.total, 3);
        assert_eq!(status.pending, 2);
        assert_eq!(status.blocked, 1);
        assert_eq!(operations.len(), 3);
        assert!(
            operations
                .iter()
                .any(|operation| operation.remote_content_hash
                    == Some("remote-test-hash".to_owned()))
        );
        assert!(operations.iter().any(|operation| operation.kind
            == SyncOperationKind::UploadToRemote
            && operation.status == SyncOperationStatus::Pending));
        assert!(operations.iter().any(|operation| operation.kind
            == SyncOperationKind::ResolveConflict
            && operation.status == SyncOperationStatus::Blocked));
        Ok(())
    }

    #[tokio::test]
    async fn replacing_sync_plan_removes_old_pending_operations() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let remote_device_id = DeviceId::new().to_string();
        let first = SyncPlan {
            local_device_id: DeviceId::new().to_string(),
            remote_device_id: remote_device_id.clone(),
            entries: vec![plan_entry("upload.txt", SyncAction::UploadToRemote)],
            summary: SyncPlanSummary::default(),
        };
        let second = SyncPlan {
            local_device_id: DeviceId::new().to_string(),
            remote_device_id,
            entries: vec![plan_entry("download.txt", SyncAction::DownloadFromRemote)],
            summary: SyncPlanSummary::default(),
        };

        database.enqueue_sync_plan(&first).await?;
        database.enqueue_sync_plan(&second).await?;
        let operations = database.list_sync_operations().await?;

        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].path.as_path().as_str(), "download.txt");
        Ok(())
    }

    #[tokio::test]
    async fn lists_and_marks_pending_download_operations() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let remote_device_id = DeviceId::new().to_string();
        let plan = SyncPlan {
            local_device_id: DeviceId::new().to_string(),
            remote_device_id: remote_device_id.clone(),
            entries: vec![
                plan_entry("download-a.txt", SyncAction::DownloadFromRemote),
                plan_entry("download-b.txt", SyncAction::DownloadFromRemote),
            ],
            summary: SyncPlanSummary::default(),
        };

        database.enqueue_sync_plan(&plan).await?;
        let downloads = database
            .list_pending_download_operations(&remote_device_id, 1)
            .await?;
        assert_eq!(downloads.len(), 1);

        database.mark_sync_operation_done(&downloads[0].id).await?;
        let remaining = database
            .list_pending_download_operations(&remote_device_id, 10)
            .await?;
        let status = database.queue_status().await?;

        assert_eq!(remaining.len(), 1);
        assert_eq!(status.done, 1);
        assert_eq!(status.pending, 1);
        Ok(())
    }

    #[tokio::test]
    async fn records_failed_sync_operation_reason() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let remote_device_id = DeviceId::new().to_string();
        let plan = SyncPlan {
            local_device_id: DeviceId::new().to_string(),
            remote_device_id,
            entries: vec![plan_entry("download.txt", SyncAction::DownloadFromRemote)],
            summary: SyncPlanSummary::default(),
        };

        database.enqueue_sync_plan(&plan).await?;
        let operation = database.list_sync_operations().await?.remove(0);
        database
            .mark_sync_operation_failed(&operation.id, "hash mismatch")
            .await?;
        let operation = database.list_sync_operations().await?.remove(0);

        assert_eq!(operation.status, SyncOperationStatus::Failed);
        assert_eq!(operation.last_error, Some("hash mismatch".to_owned()));
        Ok(())
    }

    fn plan_entry(path: &str, action: SyncAction) -> SyncPlanEntry {
        SyncPlanEntry {
            path: path.to_owned(),
            action,
            local_size_bytes: Some(5),
            remote_size_bytes: Some(7),
            local_content_hash: Some("local-test-hash".to_owned()),
            remote_content_hash: Some("remote-test-hash".to_owned()),
            reason: "test".to_owned(),
        }
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
