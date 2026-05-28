use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use time::Duration;

use crate::error::{CoreError, CoreResult};
use crate::ids::{DeviceId, FolderId};
use crate::limits::FolderSizeLimit;
use crate::security::PublicKeyBytes;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DeviceName(String);

impl DeviceName {
    /// Parses a user-visible device name.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is empty, longer than 64 characters, or
    /// contains control characters.
    pub fn parse(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into().trim().to_owned();
        let valid = !value.is_empty()
            && value.chars().count() <= 64
            && value.chars().all(|character| !character.is_control());

        if valid {
            Ok(Self(value))
        } else {
            Err(CoreError::InvalidDeviceName)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum FolderMode {
    #[default]
    Bidirectional,
    UploadOnly,
    DownloadOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DeleteScope {
    LocalOnly,
    PropagateToPeers,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetentionPolicy {
    pub max_versions_per_file: u16,
    pub max_age_days: u16,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_versions_per_file: 5,
            max_age_days: 30,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PeerEndpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderPeer {
    pub device_id: DeviceId,
    pub device_name: DeviceName,
    pub public_key: PublicKeyBytes,
    pub endpoint: Option<PeerEndpoint>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderConfig {
    pub id: FolderId,
    pub display_name: String,
    pub root: StorageRoot,
    pub mode: FolderMode,
    pub interval_seconds: u64,
    pub size_limit: FolderSizeLimit,
    pub retention: RetentionPolicy,
    pub peers: Vec<FolderPeer>,
}

impl FolderConfig {
    #[must_use]
    pub fn sync_interval(&self) -> Duration {
        Duration::seconds(self.interval_seconds.try_into().unwrap_or(i64::MAX))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum StorageRoot {
    NativePath(Utf8PathBuf),
    AndroidTreeUri(String),
}
