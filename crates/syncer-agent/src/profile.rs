use rand::random;
use serde::{Deserialize, Serialize};
use syncer_core::{DeviceId, DeviceName};
use time::OffsetDateTime;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalDeviceProfile {
    pub device_id: DeviceId,
    pub device_name: DeviceName,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub local_api_token_hex: String,
}

impl LocalDeviceProfile {
    pub fn create(name: String) -> Result<Self, syncer_core::CoreError> {
        Ok(Self {
            device_id: DeviceId::new(),
            device_name: DeviceName::parse(name)?,
            created_at: OffsetDateTime::now_utc(),
            local_api_token_hex: hex_token(random()),
        })
    }

    pub fn read(path: &camino::Utf8Path) -> Result<Self, ProfileReadError> {
        let bytes = std::fs::read(path).map_err(|source| ProfileReadError::Read {
            path: path.to_owned(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(ProfileReadError::Json)
    }
}

fn hex_token(bytes: [u8; 32]) -> String {
    hex::encode(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileReadError {
    #[error("failed to read device profile {path}: {source}")]
    Read {
        path: camino::Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse device profile JSON: {0}")]
    Json(serde_json::Error),
}
