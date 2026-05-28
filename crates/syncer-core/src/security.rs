use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::ids::DeviceId;
use crate::model::DeviceName;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PublicKeyBytes([u8; 32]);

impl PublicKeyBytes {
    /// Builds a fixed-size public key byte wrapper.
    ///
    /// # Errors
    ///
    /// Returns an error when `bytes` does not contain exactly 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> CoreResult<Self> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CoreError::InvalidPublicKeyLength)?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PairingTicket {
    pub device_id: DeviceId,
    pub device_name: DeviceName,
    pub temporary_public_key: PublicKeyBytes,
    pub endpoint: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl PairingTicket {
    /// Creates a short-lived local pairing ticket.
    ///
    /// # Errors
    ///
    /// Returns an error when `expires_at` is not later than `now`.
    pub fn new(
        device_id: DeviceId,
        device_name: DeviceName,
        temporary_public_key: PublicKeyBytes,
        endpoint: String,
        expires_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> CoreResult<Self> {
        if expires_at <= now {
            return Err(CoreError::ExpiredPairingTicket);
        }

        Ok(Self {
            device_id,
            device_name,
            temporary_public_key,
            endpoint,
            expires_at,
        })
    }

    #[must_use]
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at <= now
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PeerPresence {
    pub device_id: DeviceId,
    #[serde(with = "time::serde::rfc3339")]
    pub sent_at: OffsetDateTime,
    pub agent_version: String,
}
