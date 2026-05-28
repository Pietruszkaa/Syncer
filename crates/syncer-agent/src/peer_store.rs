use camino::{Utf8Path, Utf8PathBuf};
use rand::random;
use serde::{Deserialize, Serialize};
use syncer_core::DeviceId;
use time::{Duration, OffsetDateTime};

use crate::profile::LocalDeviceProfile;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PairingTicketDocument {
    pub schema_version: u16,
    pub device_id: DeviceId,
    pub device_name: String,
    pub endpoint: String,
    pub identity_public_hex: String,
    pub shared_secret_hex: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl PairingTicketDocument {
    pub fn create(
        profile: &LocalDeviceProfile,
        endpoint: String,
        ttl_seconds: i64,
    ) -> Result<Self, PeerStoreError> {
        if ttl_seconds <= 0 {
            return Err(PeerStoreError::InvalidTtl);
        }

        Ok(Self {
            schema_version: 1,
            device_id: profile.device_id,
            device_name: profile.device_name.as_str().to_owned(),
            endpoint,
            identity_public_hex: profile.identity_public_hex.clone(),
            shared_secret_hex: hex::encode(random::<[u8; 32]>()),
            expires_at: OffsetDateTime::now_utc() + Duration::seconds(ttl_seconds),
        })
    }

    pub fn read(path: &Utf8Path) -> Result<Self, PeerStoreError> {
        let bytes = std::fs::read(path).map_err(|source| PeerStoreError::Read {
            path: path.to_owned(),
            source,
        })?;
        let ticket: Self = serde_json::from_slice(&bytes)?;
        ticket.validate()?;
        Ok(ticket)
    }

    pub fn validate(&self) -> Result<(), PeerStoreError> {
        if self.schema_version != 1 {
            return Err(PeerStoreError::UnsupportedSchema(self.schema_version));
        }
        if self.expires_at <= OffsetDateTime::now_utc() {
            return Err(PeerStoreError::ExpiredTicket);
        }
        if hex::decode(&self.shared_secret_hex)?.len() != 32 {
            return Err(PeerStoreError::InvalidSharedSecret);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PeerStoreDocument {
    pub schema_version: u16,
    pub peers: Vec<TrustedPeer>,
}

impl PeerStoreDocument {
    pub fn read_or_default(path: &Utf8Path) -> Result<Self, PeerStoreError> {
        if !path.exists() {
            return Ok(Self {
                schema_version: 1,
                peers: Vec::new(),
            });
        }

        let bytes = std::fs::read(path).map_err(|source| PeerStoreError::Read {
            path: path.to_owned(),
            source,
        })?;
        let document: Self = serde_json::from_slice(&bytes)?;
        if document.schema_version != 1 {
            return Err(PeerStoreError::UnsupportedSchema(document.schema_version));
        }
        Ok(document)
    }

    pub fn write(&self, path: &Utf8Path) -> Result<(), PeerStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| PeerStoreError::CreateDirectory {
                path: parent.to_owned(),
                source,
            })?;
        }

        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&temporary, bytes).map_err(|source| PeerStoreError::Write {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, path).map_err(|source| PeerStoreError::Write {
            path: path.to_owned(),
            source,
        })?;
        Ok(())
    }

    pub fn accept_ticket(&mut self, ticket: PairingTicketDocument) -> Result<(), PeerStoreError> {
        ticket.validate()?;
        let peer = TrustedPeer {
            device_id: ticket.device_id,
            device_name: ticket.device_name,
            endpoint: ticket.endpoint,
            identity_public_hex: ticket.identity_public_hex,
            shared_secret_hex: ticket.shared_secret_hex,
            trusted_at: OffsetDateTime::now_utc(),
        };

        if let Some(existing) = self
            .peers
            .iter_mut()
            .find(|existing| existing.device_id == peer.device_id)
        {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
        Ok(())
    }

    pub fn trusted_peer(&self, device_id: DeviceId) -> Option<&TrustedPeer> {
        self.peers.iter().find(|peer| peer.device_id == device_id)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrustedPeer {
    pub device_id: DeviceId,
    pub device_name: String,
    pub endpoint: String,
    pub identity_public_hex: String,
    pub shared_secret_hex: String,
    #[serde(with = "time::serde::rfc3339")]
    pub trusted_at: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerStoreError {
    #[error("ticket ttl must be greater than zero")]
    InvalidTtl,
    #[error("pairing ticket has expired")]
    ExpiredTicket,
    #[error("unsupported peer document schema {0}")]
    UnsupportedSchema(u16),
    #[error("shared secret must be 32 bytes encoded as hex")]
    InvalidSharedSecret,
    #[error("failed to read {path}: {source}")]
    Read {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create directory {path}: {source}")]
    CreateDirectory {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),
}
