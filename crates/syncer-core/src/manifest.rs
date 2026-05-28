use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::ids::DeviceId;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RelativePath(Utf8PathBuf);

impl RelativePath {
    /// Parses a safe user-file relative path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is empty, absolute, contains traversal
    /// components, or targets Syncer's `.syncer` metadata directory.
    pub fn parse(value: impl AsRef<str>) -> CoreResult<Self> {
        let raw = value.as_ref().trim();
        if raw.is_empty() {
            return Err(CoreError::EmptyRelativePath);
        }

        let path = Utf8Path::new(raw);
        if path.is_absolute() {
            return Err(CoreError::AbsoluteRelativePath);
        }

        let mut normalized = Utf8PathBuf::new();
        for component in path.components() {
            match component {
                Utf8Component::Normal(part) => normalized.push(part),
                Utf8Component::CurDir => {}
                Utf8Component::ParentDir => return Err(CoreError::TraversalRelativePath),
                Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                    return Err(CoreError::AbsoluteRelativePath);
                }
            }
        }

        if normalized.as_str().is_empty() {
            return Err(CoreError::EmptyRelativePath);
        }

        if normalized
            .components()
            .next()
            .is_some_and(|component| component.as_str() == ".syncer")
        {
            return Err(CoreError::SyncerMetadataPath);
        }

        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_path(&self) -> &Utf8Path {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    #[must_use]
    pub fn from_hex(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    #[must_use]
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BlockHash(String);

impl BlockHash {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileVersion {
    pub generation: u64,
    pub device_id: DeviceId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileEntry {
    pub path: RelativePath,
    pub kind: FileKind,
    pub size_bytes: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub modified_at: OffsetDateTime,
    pub content_hash: Option<ContentHash>,
    pub block_hashes: Vec<BlockHash>,
    pub version: FileVersion,
}

#[cfg(test)]
mod tests {
    use crate::CoreError;

    use super::RelativePath;

    #[test]
    fn accepts_normal_relative_paths() -> Result<(), CoreError> {
        let path = RelativePath::parse("photos/2026/image.jpg")?;

        assert_eq!(path.as_path().as_str(), "photos/2026/image.jpg");
        Ok(())
    }

    #[test]
    fn rejects_metadata_paths() {
        let error = RelativePath::parse(".syncer/state.db");

        assert_eq!(error, Err(CoreError::SyncerMetadataPath));
    }

    #[test]
    fn rejects_traversal() {
        let error = RelativePath::parse("photos/../secret.txt");

        assert_eq!(error, Err(CoreError::TraversalRelativePath));
    }
}
