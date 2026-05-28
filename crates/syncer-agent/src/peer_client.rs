use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use syncer_core::{FolderManifest, PeerPresence, RelativePath};
use time::OffsetDateTime;

use crate::path_codec::encode_relative_path;
use crate::peer_server::PresenceResponse;
use crate::request_signing::sign_request;

const RESPONSE_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const RESPONSE_HEADER_LIMIT_BYTES: u64 = 16 * 1024;

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

pub fn fetch_file(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
    path: &RelativePath,
    size_bytes: u64,
) -> Result<Vec<u8>, PeerClientError> {
    let request_path = format!("/file/{}", encode_relative_path(path));
    request_bytes(&PeerRequest {
        method: "GET",
        endpoint,
        path: &request_path,
        device_id,
        shared_secret_hex,
        body: &[],
        extra_headers: &[],
        response_limit_bytes: size_bytes.saturating_add(RESPONSE_HEADER_LIMIT_BYTES),
    })
    .and_then(|response| parse_bytes_response(&response))
}

pub fn put_file(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
    path: &RelativePath,
    content_hash: &str,
    bytes: &[u8],
) -> Result<UploadFileResponse, PeerClientError> {
    let request_path = format!("/file/{}", encode_relative_path(path));
    let extra_headers = vec![
        ("x-syncer-content-hash".to_owned(), content_hash.to_owned()),
        ("x-syncer-file-size".to_owned(), bytes.len().to_string()),
    ];
    request_json_with_body(
        "PUT",
        endpoint,
        &request_path,
        device_id,
        shared_secret_hex,
        bytes,
        &extra_headers,
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
    let body = match body {
        Some(value) => serde_json::to_vec(value)?,
        None => Vec::new(),
    };
    let response = request_bytes(&PeerRequest {
        method,
        endpoint,
        path,
        device_id,
        shared_secret_hex,
        body: &body,
        extra_headers: &[],
        response_limit_bytes: RESPONSE_LIMIT_BYTES as u64,
    })?;

    parse_json_response(&response)
}

fn request_json_with_body<R: DeserializeOwned>(
    method: &str,
    endpoint: &str,
    path: &str,
    device_id: &str,
    shared_secret_hex: &str,
    body: &[u8],
    extra_headers: &[(String, String)],
) -> Result<R, PeerClientError> {
    let response = request_bytes(&PeerRequest {
        method,
        endpoint,
        path,
        device_id,
        shared_secret_hex,
        body,
        extra_headers,
        response_limit_bytes: RESPONSE_LIMIT_BYTES as u64,
    })?;

    parse_json_response(&response)
}

fn request_bytes(request: &PeerRequest<'_>) -> Result<Vec<u8>, PeerClientError> {
    let address = PeerAddress::parse(request.endpoint)?;
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let signature = sign_request(
        request.method,
        request.path,
        request.device_id,
        timestamp,
        request.body,
        request.shared_secret_hex,
    )?;
    let mut serialized_request = format!(
        "{} {} HTTP/1.1\r\nhost: {}\r\nx-syncer-device-id: {}\r\nx-syncer-timestamp: {timestamp}\r\nx-syncer-signature: {signature}\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\n",
        request.method,
        request.path,
        address.host,
        request.device_id,
        request.body.len()
    );
    for (name, value) in request.extra_headers {
        serialized_request.push_str(name);
        serialized_request.push_str(": ");
        serialized_request.push_str(value);
        serialized_request.push_str("\r\n");
    }
    serialized_request.push_str("connection: close\r\n\r\n");

    let mut stream = TcpStream::connect((&address.host[..], address.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(serialized_request.as_bytes())?;
    stream.write_all(request.body)?;

    let mut response = Vec::new();
    stream
        .take(request.response_limit_bytes)
        .read_to_end(&mut response)?;
    Ok(response)
}

#[derive(Debug)]
struct PeerRequest<'request> {
    method: &'request str,
    endpoint: &'request str,
    path: &'request str,
    device_id: &'request str,
    shared_secret_hex: &'request str,
    body: &'request [u8],
    extra_headers: &'request [(String, String)],
    response_limit_bytes: u64,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct UploadFileResponse {
    pub accepted: bool,
    pub path: RelativePath,
    pub size_bytes: u64,
    pub content_hash: String,
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

fn parse_bytes_response(response: &[u8]) -> Result<Vec<u8>, PeerClientError> {
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

    if status != 200 {
        return Err(PeerClientError::HttpStatus(status));
    }

    Ok(response[header_end + 4..].to_vec())
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
