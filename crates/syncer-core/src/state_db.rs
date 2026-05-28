use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::manifest::{ContentHash, FileEntry, FileKind, RelativePath};

const SCHEMA_VERSION: i64 = 1;

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

    /// Inserts or updates a scanned file entry.
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
              block_hashes_json, generation, device_id
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(path) DO UPDATE SET
              kind = excluded.kind,
              size_bytes = excluded.size_bytes,
              modified_at_unix = excluded.modified_at_unix,
              content_hash = excluded.content_hash,
              block_hashes_json = excluded.block_hashes_json,
              generation = excluded.generation,
              device_id = excluded.device_id,
              scan_missing = 0,
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

    /// Marks all known files as missing before a scan.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the update.
    pub async fn mark_scan_started(&self) -> CoreResult<()> {
        sqlx::query("UPDATE files SET scan_missing = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Lists files that were known before scan but not seen during scan.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` rejects the query.
    pub async fn list_missing_after_scan(&self) -> CoreResult<Vec<FileRecord>> {
        let rows = sqlx::query(
            r"
            SELECT path, kind, size_bytes, modified_at_unix, content_hash, generation, device_id
            FROM files
            WHERE scan_missing = 1
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
            SELECT path, kind, size_bytes, modified_at_unix, content_hash, generation, device_id
            FROM files
            ORDER BY path
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_file_record).collect()
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
              updated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
            )
            ",
        )
        .execute(&self.pool)
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
}

fn row_to_file_record(row: &sqlx::sqlite::SqliteRow) -> CoreResult<FileRecord> {
    let path: String = row.try_get("path")?;
    let kind: String = row.try_get("kind")?;
    let size_bytes: i64 = row.try_get("size_bytes")?;
    let modified_at_unix: i64 = row.try_get("modified_at_unix")?;
    let content_hash: Option<String> = row.try_get("content_hash")?;
    let generation: i64 = row.try_get("generation")?;
    let device_id: String = row.try_get("device_id")?;

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
    })
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use crate::{
        ContentHash, DeviceId, FileEntry, FileKind, FileVersion, RelativePath, StateDatabase,
    };

    #[tokio::test]
    async fn stores_and_lists_file_entries() -> Result<(), crate::CoreError> {
        let database = StateDatabase::in_memory().await?;
        let entry = FileEntry {
            path: RelativePath::parse("docs/readme.txt")?,
            kind: FileKind::File,
            size_bytes: 5,
            modified_at: datetime!(2026-05-28 10:00 UTC),
            content_hash: Some(ContentHash::from_bytes(b"hello")),
            block_hashes: Vec::new(),
            version: FileVersion {
                generation: 1,
                device_id: DeviceId::new(),
            },
        };

        database.upsert_file(&entry).await?;
        let files = database.list_files().await?;

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_path().as_str(), "docs/readme.txt");
        Ok(())
    }
}
