use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use camino::Utf8Path;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use syncer_core::{FolderManifest, PeerPresence, RelativePath};
use time::OffsetDateTime;

use crate::path_codec::encode_relative_path;
use crate::peer_crypto::{
    ENCRYPTION_ALGORITHM, FileCipherContext, decrypt_file_to_path, encrypt_file_to_path,
    encrypted_len,
};
use crate::peer_server::PresenceResponse;
use crate::request_signing::sign_request;

const RESPONSE_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const RESPONSE_HEADER_LIMIT_BYTES: usize = 16 * 1024;

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
        &[],
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
        &[],
    )
}

pub fn fetch_file_to_path(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
    path: &RelativePath,
    size_bytes: u64,
    destination: &Utf8Path,
) -> Result<TransferredFile, PeerClientError> {
    let request_path = format!("/file/{}", encode_relative_path(path));
    let address = PeerAddress::parse(endpoint)?;
    let request = signed_request_header(&PeerRequest {
        method: "GET",
        endpoint,
        path: &request_path,
        device_id,
        shared_secret_hex,
        body: &[],
        extra_headers: &[],
        response_limit_bytes: RESPONSE_LIMIT_BYTES as u64,
    })?;
    let mut stream = TcpStream::connect((&address.host[..], address.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;

    let head = read_response_head(&mut stream)?;
    if head.status != 200 {
        return Err(PeerClientError::HttpStatus(head.status));
    }
    let encryption = head
        .headers
        .get("x-syncer-encryption")
        .ok_or(PeerClientError::InvalidResponse)?;
    if encryption != ENCRYPTION_ALGORITHM {
        return Err(PeerClientError::InvalidResponse);
    }
    let nonce = head
        .headers
        .get("x-syncer-nonce")
        .ok_or(PeerClientError::InvalidResponse)?;
    let content_hash = head
        .headers
        .get("x-syncer-content-hash")
        .ok_or(PeerClientError::InvalidResponse)?;
    let expected_ciphertext_size = encrypted_len(size_bytes);
    if head.content_length != expected_ciphertext_size {
        return Err(PeerClientError::UnexpectedContentLength {
            expected: expected_ciphertext_size,
            actual: head.content_length,
        });
    }

    let encrypted = encrypted_download_path(destination);
    let mut output = File::create(&encrypted)?;
    copy_exact(&mut stream, &mut output, head.content_length)?;
    output.sync_all()?;
    let decrypted = decrypt_file_to_path(
        &encrypted,
        destination,
        shared_secret_hex,
        nonce,
        &FileCipherContext {
            direction: "download",
            path,
            content_hash,
            plaintext_size: size_bytes,
        },
    )?;
    std::fs::remove_file(&encrypted).ok();

    Ok(TransferredFile {
        size_bytes: decrypted.size_bytes,
        content_hash: decrypted.content_hash,
    })
}

pub fn put_file_from_path(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
    path: &RelativePath,
    content_hash: &str,
    file_path: &Utf8Path,
    size_bytes: u64,
) -> Result<UploadFileResponse, PeerClientError> {
    let request_path = format!("/file/{}", encode_relative_path(path));
    let encrypted = encrypted_upload_path(file_path);
    let encrypted_file = encrypt_file_to_path(
        file_path,
        &encrypted,
        shared_secret_hex,
        &FileCipherContext {
            direction: "upload",
            path,
            content_hash,
            plaintext_size: size_bytes,
        },
    )?;
    if encrypted_file.content_hash != content_hash {
        std::fs::remove_file(&encrypted).ok();
        return Err(PeerClientError::PlaintextHashMismatch);
    }
    let body_sha256_hex = file_sha256_hex(&encrypted)?;
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let signature = crate::request_signing::sign_request_with_body_hash_hex(
        "PUT",
        &request_path,
        device_id,
        timestamp,
        &body_sha256_hex,
        shared_secret_hex,
    )?;
    let address = PeerAddress::parse(endpoint)?;
    let request = format!(
        "PUT {request_path} HTTP/1.1\r\nhost: {}\r\nx-syncer-device-id: {device_id}\r\nx-syncer-timestamp: {timestamp}\r\nx-syncer-signature: {signature}\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nx-syncer-content-hash: {content_hash}\r\nx-syncer-file-size: {size_bytes}\r\nx-syncer-encryption: {ENCRYPTION_ALGORITHM}\r\nx-syncer-nonce: {}\r\nconnection: close\r\n\r\n",
        address.host, encrypted_file.size_bytes, encrypted_file.nonce_hex
    );
    let mut stream = TcpStream::connect((&address.host[..], address.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    let mut file = File::open(&encrypted)?;
    std::io::copy(&mut file, &mut stream)?;
    stream.flush()?;
    std::fs::remove_file(&encrypted).ok();

    let mut response = Vec::new();
    stream
        .take(RESPONSE_LIMIT_BYTES as u64)
        .read_to_end(&mut response)?;
    parse_json_response(&response)
}

pub fn delete_file(
    endpoint: &str,
    device_id: &str,
    shared_secret_hex: &str,
    path: &RelativePath,
    content_hash: &str,
    size_bytes: u64,
) -> Result<DeleteFileResponse, PeerClientError> {
    let request_path = format!("/file/{}", encode_relative_path(path));
    request_json::<(), DeleteFileResponse>(
        "DELETE",
        endpoint,
        &request_path,
        device_id,
        shared_secret_hex,
        None,
        &[
            ("x-syncer-content-hash".to_owned(), content_hash.to_owned()),
            ("x-syncer-file-size".to_owned(), size_bytes.to_string()),
        ],
    )
}

fn request_json<T: Serialize, R: DeserializeOwned>(
    method: &str,
    endpoint: &str,
    path: &str,
    device_id: &str,
    shared_secret_hex: &str,
    body: Option<&T>,
    extra_headers: &[(String, String)],
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
        extra_headers,
        response_limit_bytes: RESPONSE_LIMIT_BYTES as u64,
    })?;

    parse_json_response(&response)
}

fn request_bytes(request: &PeerRequest<'_>) -> Result<Vec<u8>, PeerClientError> {
    let address = PeerAddress::parse(request.endpoint)?;
    let serialized_request = signed_request_header(request)?;

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

fn signed_request_header(request: &PeerRequest<'_>) -> Result<String, PeerClientError> {
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
    Ok(serialized_request)
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

#[derive(Debug, serde::Deserialize)]
pub struct DeleteFileResponse {
    pub accepted: bool,
    pub path: RelativePath,
    pub size_bytes: u64,
    pub content_hash: String,
}

#[derive(Clone, Debug)]
pub struct TransferredFile {
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

fn read_response_head(reader: &mut impl Read) -> Result<ResponseHead, PeerClientError> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        reader.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.len() > RESPONSE_HEADER_LIMIT_BYTES {
            return Err(PeerClientError::InvalidResponse);
        }
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(PeerClientError::InvalidResponse)?;
    let header =
        std::str::from_utf8(&bytes[..header_end]).map_err(|_| PeerClientError::InvalidResponse)?;
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or(PeerClientError::InvalidResponse)?
        .parse::<u16>()
        .map_err(|_| PeerClientError::InvalidResponse)?;
    let headers = parse_headers(header);
    let content_length = headers
        .get("content-length")
        .ok_or(PeerClientError::InvalidResponse)?
        .parse::<u64>()
        .map_err(|_| PeerClientError::InvalidResponse)?;
    Ok(ResponseHead {
        status,
        content_length,
        headers,
    })
}

fn parse_headers(header: &str) -> HashMap<String, String> {
    header
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect()
}

fn copy_exact(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected: u64,
) -> Result<u64, PeerClientError> {
    let mut remaining = expected;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let read_limit = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| PeerClientError::InvalidResponse)?;
        let read = reader.read(&mut buffer[..read_limit])?;
        if read == 0 {
            return Err(PeerClientError::InvalidResponse);
        }
        writer.write_all(&buffer[..read])?;
        remaining = remaining.saturating_sub(read as u64);
    }
    Ok(expected)
}

fn encrypted_upload_path(path: &Utf8Path) -> camino::Utf8PathBuf {
    path.with_extension(format!("syncer-upload-{}.enc", uuid::Uuid::now_v7()))
}

fn encrypted_download_path(path: &Utf8Path) -> camino::Utf8PathBuf {
    path.with_extension(format!("syncer-download-{}.enc", uuid::Uuid::now_v7()))
}

fn file_sha256_hex(path: &Utf8Path) -> Result<String, PeerClientError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[derive(Debug)]
struct ResponseHead {
    status: u16,
    content_length: u64,
    headers: HashMap<String, String>,
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
    #[error("unexpected content length: expected {expected}, got {actual}")]
    UnexpectedContentLength { expected: u64, actual: u64 },
    #[error("plaintext hash does not match the queued content hash")]
    PlaintextHashMismatch,
    #[error("{0}")]
    PeerCrypto(#[from] crate::peer_crypto::PeerCryptoError),
    #[error("{0}")]
    Signing(#[from] crate::request_signing::SigningError),
}
