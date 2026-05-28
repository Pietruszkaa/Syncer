use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use syncer_core::{FolderManifest, PeerPresence};
use time::OffsetDateTime;

use crate::peer_server::PresenceResponse;
use crate::request_signing::sign_request;

const RESPONSE_LIMIT_BYTES: usize = 4 * 1024 * 1024;

pub fn post_presence(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
    presence: &PeerPresence,
) -> Result<PresenceResponse, PeerClientError> {
    request_json(
        "POST",
        endpoint,
        "/presence",
        device_id,
        shared_secret_hex,
        Some(presence),
    )
}

pub fn fetch_manifest(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
) -> Result<FolderManifest, PeerClientError> {
    request_json::<(), FolderManifest>(
        "GET",
        endpoint,
        "/manifest",
        device_id,
        shared_secret_hex,
        None,
    )
}

fn request_json<T: Serialize, R: DeserializeOwned>(
    method: &str,
    endpoint: &str,
    path: &str,
    device_id: &str,
    shared_secret_hex: &str,
    body: Option<&T>,
) -> Result<R, PeerClientError> {
    let address = PeerAddress::parse(endpoint)?;
    let body = match body {
        Some(value) => serde_json::to_vec(value)?,
        None => Vec::new(),
    };
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let signature = sign_request(method, path, device_id, timestamp, &body, shared_secret_hex)?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {}\r\nx-syncer-device-id: {device_id}\r\nx-syncer-timestamp: {timestamp}\r\nx-syncer-signature: {signature}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        address.host,
        body.len()
    );

    let mut stream = TcpStream::connect((&address.host[..], address.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(&body)?;

    let mut response = Vec::new();
    stream
        .take(RESPONSE_LIMIT_BYTES as u64)
        .read_to_end(&mut response)?;
    parse_json_response(&response)
}

fn parse_json_response<R: DeserializeOwned>(response: &[u8]) -> Result<R, PeerClientError> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(PeerClientError::InvalidResponse)?;
    let status_line = std::str::from_utf8(&response[..header_end])
        .map_err(|_| PeerClientError::InvalidResponse)?
        .lines()
        .next()
        .ok_or(PeerClientError::InvalidResponse)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(PeerClientError::InvalidResponse)?
        .parse::<u16>()
        .map_err(|_| PeerClientError::InvalidResponse)?;
    let body = &response[header_end + 4..];

    if status != 200 {
        return Err(PeerClientError::HttpStatus(status));
    }

    serde_json::from_slice(body).map_err(PeerClientError::Json)
}

#[derive(Debug)]
struct PeerAddress {
    host: String,
    port: u16,
}

impl PeerAddress {
    fn parse(endpoint: &str) -> Result<Self, PeerClientError> {
        let endpoint = endpoint
            .strip_prefix("http://")
            .ok_or(PeerClientError::UnsupportedEndpoint)?;
        let (host, port) = endpoint
            .rsplit_once(':')
            .ok_or(PeerClientError::UnsupportedEndpoint)?;
        let port = port
            .parse::<u16>()
            .map_err(|_| PeerClientError::UnsupportedEndpoint)?;
        if host.is_empty() {
            return Err(PeerClientError::UnsupportedEndpoint);
        }

        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerClientError {
    #[error("endpoint must use http://host:port")]
    UnsupportedEndpoint,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid HTTP response")]
    InvalidResponse,
    #[error("peer returned HTTP {0}")]
    HttpStatus(u16),
    #[error("{0}")]
    Signing(#[from] crate::request_signing::SigningError),
}
