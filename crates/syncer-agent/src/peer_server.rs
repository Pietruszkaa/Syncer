use std::collections::HashMap;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use syncer_core::{
    ContentHash, FileEntry, FileKind, FileSyncState, FileVersion, FolderManifest, FolderStore,
    PeerPresence, RelativePath, StateDatabase,
};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Duration, timeout};

use crate::path_codec::decode_relative_path;
use crate::peer_store::PeerStoreDocument;
use crate::profile::LocalDeviceProfile;
use crate::request_signing::verify_request_signature_with_body_hash_hex;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_MEMORY_BODY_BYTES: u64 = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct PeerServerConfig {
    pub bind: String,
    pub folder_path: Utf8PathBuf,
    pub peers_path: Utf8PathBuf,
    pub profile: LocalDeviceProfile,
}

pub async fn run(config: PeerServerConfig) -> Result<(), PeerServerError> {
    let listener = TcpListener::bind(&config.bind).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let config = config.clone();
        tokio::spawn(async move {
            let _ = handle_stream(stream, config).await;
        });
    }
}

async fn handle_stream(
    mut stream: TcpStream,
    config: PeerServerConfig,
) -> Result<(), PeerServerError> {
    let response = match read_request(&mut stream, &config).await {
        Ok(request) => route_request(&request, &config)
            .await
            .unwrap_or_else(|error| error_response(&error)),
        Err(error) => error_response(&error),
    };
    write_response(&mut stream, response).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn route_request(
    request: &HttpRequest,
    config: &PeerServerConfig,
) -> Result<PeerResponse, PeerServerError> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => json_response(
            200,
            &HealthResponse {
                ok: true,
                device_id: config.profile.device_id.to_string(),
            },
        ),
        ("POST", "/presence") => {
            verify_peer_request(request, &config.peers_path)?;
            let presence: PeerPresence = serde_json::from_slice(request.memory_body()?)?;
            json_response(
                200,
                &PresenceResponse {
                    accepted: true,
                    received_from: presence.device_id.to_string(),
                    local_presence: PeerPresence {
                        device_id: config.profile.device_id,
                        sent_at: OffsetDateTime::now_utc(),
                        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
                    },
                },
            )
        }
        ("GET", "/manifest") => {
            verify_peer_request(request, &config.peers_path)?;
            let manifest = export_manifest(&config.folder_path, &config.profile).await?;
            json_response(200, &manifest)
        }
        ("GET", path) if path.starts_with("/file/") => {
            verify_peer_request(request, &config.peers_path)?;
            let relative_path = decode_relative_path(path.trim_start_matches("/file/"))
                .map_err(|_| PeerServerError::InvalidRequest)?;
            let shared_file =
                shared_file(&config.folder_path, &config.profile, &relative_path).await?;
            Ok(file_response(
                200,
                "application/octet-stream",
                shared_file.path,
                shared_file.size_bytes,
            ))
        }
        ("PUT", path) if path.starts_with("/file/") => {
            verify_peer_request(request, &config.peers_path)?;
            let relative_path = decode_relative_path(path.trim_start_matches("/file/"))
                .map_err(|_| PeerServerError::InvalidRequest)?;
            let expected_size = required_u64_header(request, "x-syncer-file-size")?;
            let expected_hash = required_header(request, "x-syncer-content-hash")?;
            let response = write_uploaded_file(
                &config.folder_path,
                &config.profile,
                &relative_path,
                expected_size,
                expected_hash,
                request.file_body()?,
            )
            .await?;
            json_response(200, &response)
        }
        _ => json_response(
            404,
            &ErrorResponse {
                error: "not_found".to_owned(),
            },
        ),
    }
}

async fn shared_file(
    folder_path: &Utf8PathBuf,
    profile: &LocalDeviceProfile,
    relative_path: &syncer_core::RelativePath,
) -> Result<SharedFile, PeerServerError> {
    let manifest = export_manifest(folder_path, profile).await?;
    let Some(file) = manifest
        .files
        .iter()
        .find(|file| file.path == *relative_path)
    else {
        return Err(PeerServerError::NotFound);
    };
    if file.kind != FileKind::File || file.sync_state != FileSyncState::LocalAvailable {
        return Err(PeerServerError::NotFound);
    }

    let path = folder_path.join(relative_path.as_path());
    Ok(SharedFile {
        path,
        size_bytes: file.size_bytes,
    })
}

async fn write_uploaded_file(
    folder_path: &Utf8PathBuf,
    profile: &LocalDeviceProfile,
    relative_path: &RelativePath,
    expected_size: u64,
    expected_hash: &str,
    body: FileBody,
) -> Result<UploadFileResponse, PeerServerError> {
    if body.size_bytes != expected_size {
        return Err(PeerServerError::InvalidRequest);
    }
    let content_hash = blake3_file_hex(&body.path).await?;
    if content_hash != expected_hash {
        return Err(PeerServerError::HashMismatch);
    }

    let store = FolderStore::new(folder_path.clone());
    enforce_folder_limit(&store, expected_size).await?;
    let target = folder_path.join(relative_path.as_path());
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|source| PeerServerError::Filesystem {
            path: target.clone(),
            source,
        })?
    {
        return Err(PeerServerError::Conflict);
    }

    publish_uploaded_file(&store, relative_path, &body.path).await?;
    store.read().await?;
    let database = StateDatabase::open(&store.state_db_path()).await?;
    database
        .upsert_file(&FileEntry {
            path: relative_path.clone(),
            kind: FileKind::File,
            size_bytes: expected_size,
            modified_at: OffsetDateTime::now_utc(),
            content_hash: Some(ContentHash::from_hex(content_hash.clone())),
            block_hashes: Vec::new(),
            version: FileVersion {
                generation: 1,
                device_id: profile.device_id,
            },
        })
        .await?;

    Ok(UploadFileResponse {
        accepted: true,
        path: relative_path.clone(),
        size_bytes: expected_size,
        content_hash: expected_hash.to_owned(),
    })
}

async fn enforce_folder_limit(
    store: &FolderStore,
    incoming_size: u64,
) -> Result<(), PeerServerError> {
    let folder = store.read().await?;
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let status = database.folder_status().await?;
    let remaining = folder
        .folder
        .size_limit
        .remaining_bytes(status.indexed_bytes);
    if incoming_size > remaining {
        return Err(PeerServerError::FolderLimitExceeded {
            remaining,
            requested: incoming_size,
        });
    }
    Ok(())
}

async fn blake3_file_hex(path: &camino::Utf8Path) -> Result<String, PeerServerError> {
    let mut file =
        tokio::fs::File::open(path)
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: path.to_owned(),
                source,
            })?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: path.to_owned(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

async fn publish_uploaded_file(
    store: &FolderStore,
    relative_path: &RelativePath,
    temporary: &camino::Utf8Path,
) -> Result<(), PeerServerError> {
    let target = store.root().join(relative_path.as_path());
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: parent.to_owned(),
                source,
            })?;
    }
    tokio::fs::rename(temporary, &target)
        .await
        .map_err(|source| PeerServerError::Filesystem {
            path: target,
            source,
        })
}

async fn export_manifest(
    folder_path: &Utf8PathBuf,
    profile: &LocalDeviceProfile,
) -> Result<FolderManifest, PeerServerError> {
    let store = FolderStore::new(folder_path.clone());
    let folder = store.read().await?;
    let database = StateDatabase::open(&store.state_db_path()).await?;
    database
        .export_manifest(folder.folder.id, profile.device_id)
        .await
        .map_err(PeerServerError::Core)
}

async fn read_request(
    stream: &mut TcpStream,
    config: &PeerServerConfig,
) -> Result<HttpRequest, PeerServerError> {
    let mut buffer = Vec::new();
    let mut temporary = [0_u8; 4096];

    loop {
        let read = timeout(REQUEST_TIMEOUT, stream.read(&mut temporary))
            .await
            .map_err(|_| PeerServerError::RequestTimeout)??;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temporary[..read]);
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(PeerServerError::RequestTooLarge);
        }
        if find_header_end(&buffer).is_some() {
            break;
        }
    }

    parse_request(stream, config, &buffer).await
}

async fn parse_request(
    stream: &mut TcpStream,
    config: &PeerServerConfig,
    buffer: &[u8],
) -> Result<HttpRequest, PeerServerError> {
    let header_end = find_header_end(buffer).ok_or(PeerServerError::InvalidRequest)?;
    let header_bytes = &buffer[..header_end];
    let headers = parse_headers(header_bytes)?;
    let request_line = std::str::from_utf8(header_bytes)
        .map_err(|_| PeerServerError::InvalidRequest)?
        .lines()
        .next()
        .ok_or(PeerServerError::InvalidRequest)?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or(PeerServerError::InvalidRequest)?
        .to_owned();
    let path = request_parts
        .next()
        .ok_or(PeerServerError::InvalidRequest)?
        .split('?')
        .next()
        .ok_or(PeerServerError::InvalidRequest)?
        .to_owned();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    let body_prefix = &buffer[body_start..];
    let body = if should_spool_body(&method, &path, content_length) {
        read_file_body(stream, config, body_prefix, content_length).await?
    } else {
        read_memory_body(stream, body_prefix, content_length).await?
    };
    let body_sha256_hex = body.sha256_hex();

    Ok(HttpRequest {
        method,
        path,
        headers,
        body_sha256_hex,
        body,
    })
}

fn should_spool_body(method: &str, path: &str, content_length: u64) -> bool {
    method == "PUT" && path.starts_with("/file/") && content_length > 0
}

async fn read_memory_body(
    stream: &mut TcpStream,
    body_prefix: &[u8],
    content_length: u64,
) -> Result<HttpBody, PeerServerError> {
    if content_length > MAX_MEMORY_BODY_BYTES {
        return Err(PeerServerError::RequestTooLarge);
    }
    let content_length_usize =
        usize::try_from(content_length).map_err(|_| PeerServerError::RequestTooLarge)?;
    if body_prefix.len() > content_length_usize {
        return Err(PeerServerError::InvalidRequest);
    }

    let mut body = Vec::with_capacity(content_length_usize);
    body.extend_from_slice(body_prefix);
    while body.len() < content_length_usize {
        let remaining = content_length_usize.saturating_sub(body.len());
        let mut chunk = vec![0_u8; remaining.min(4096)];
        let read = timeout(REQUEST_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| PeerServerError::RequestTimeout)??;
        if read == 0 {
            return Err(PeerServerError::InvalidRequest);
        }
        body.extend_from_slice(&chunk[..read]);
    }

    Ok(HttpBody::Memory(body))
}

async fn read_file_body(
    stream: &mut TcpStream,
    config: &PeerServerConfig,
    body_prefix: &[u8],
    content_length: u64,
) -> Result<HttpBody, PeerServerError> {
    if u64::try_from(body_prefix.len()).map_err(|_| PeerServerError::RequestTooLarge)?
        > content_length
    {
        return Err(PeerServerError::InvalidRequest);
    }

    let temporary_dir = FolderStore::new(config.folder_path.clone())
        .syncer_dir()
        .join("tmp")
        .join("requests");
    tokio::fs::create_dir_all(&temporary_dir)
        .await
        .map_err(|source| PeerServerError::Filesystem {
            path: temporary_dir.clone(),
            source,
        })?;
    let path = temporary_dir.join(format!("{}.body", uuid::Uuid::now_v7()));
    let mut file =
        tokio::fs::File::create(&path)
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: path.clone(),
                source,
            })?;
    let mut hasher = Sha256::new();
    if !body_prefix.is_empty() {
        file.write_all(body_prefix)
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: path.clone(),
                source,
            })?;
        hasher.update(body_prefix);
    }

    let mut written =
        u64::try_from(body_prefix.len()).map_err(|_| PeerServerError::RequestTooLarge)?;
    let mut chunk = vec![0_u8; 64 * 1024];
    while written < content_length {
        let remaining = content_length.saturating_sub(written);
        let read_limit = usize::try_from(remaining.min(chunk.len() as u64))
            .map_err(|_| PeerServerError::RequestTooLarge)?;
        let read = timeout(REQUEST_TIMEOUT, stream.read(&mut chunk[..read_limit]))
            .await
            .map_err(|_| PeerServerError::RequestTimeout)??;
        if read == 0 {
            return Err(PeerServerError::InvalidRequest);
        }
        file.write_all(&chunk[..read])
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: path.clone(),
                source,
            })?;
        hasher.update(&chunk[..read]);
        written = written.saturating_add(read as u64);
    }
    file.sync_all()
        .await
        .map_err(|source| PeerServerError::Filesystem {
            path: path.clone(),
            source,
        })?;

    Ok(HttpBody::File(FileBody {
        path,
        size_bytes: content_length,
        sha256_hex: hex::encode(hasher.finalize()),
    }))
}

fn parse_headers(header_bytes: &[u8]) -> Result<HashMap<String, String>, PeerServerError> {
    let text = std::str::from_utf8(header_bytes).map_err(|_| PeerServerError::InvalidRequest)?;
    let mut headers = HashMap::new();
    for line in text.lines().skip(1) {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Ok(headers)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn verify_peer_request(
    request: &HttpRequest,
    peers_path: &Utf8PathBuf,
) -> Result<(), PeerServerError> {
    let device_id = request
        .headers
        .get("x-syncer-device-id")
        .ok_or(PeerServerError::Unauthorized)?;
    let timestamp = request
        .headers
        .get("x-syncer-timestamp")
        .ok_or(PeerServerError::Unauthorized)?
        .parse::<i64>()
        .map_err(|_| PeerServerError::Unauthorized)?;
    let signature = request
        .headers
        .get("x-syncer-signature")
        .ok_or(PeerServerError::Unauthorized)?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    if now.abs_diff(timestamp) > 300 {
        return Err(PeerServerError::Unauthorized);
    }

    let device_id = uuid::Uuid::parse_str(device_id).map_err(|_| PeerServerError::Unauthorized)?;
    let device_id = syncer_core::DeviceId::from_uuid(device_id);
    let peers = PeerStoreDocument::read_or_default(peers_path)?;
    let peer = peers
        .trusted_peer(device_id)
        .ok_or(PeerServerError::Unauthorized)?;
    verify_request_signature_with_body_hash_hex(
        signature,
        &request.method,
        &request.path,
        &device_id.to_string(),
        timestamp,
        &request.body_sha256_hex,
        &peer.shared_secret_hex,
    )?;
    Ok(())
}

fn required_header<'request>(
    request: &'request HttpRequest,
    name: &str,
) -> Result<&'request str, PeerServerError> {
    request
        .headers
        .get(name)
        .map(String::as_str)
        .ok_or(PeerServerError::InvalidRequest)
}

fn required_u64_header(request: &HttpRequest, name: &str) -> Result<u64, PeerServerError> {
    required_header(request, name)?
        .parse::<u64>()
        .map_err(|_| PeerServerError::InvalidRequest)
}

fn json_response<T: Serialize>(status: u16, value: &T) -> Result<PeerResponse, PeerServerError> {
    let body = serde_json::to_vec(value)?;
    Ok(PeerResponse::Bytes(binary_response(
        status,
        "application/json",
        &body,
    )))
}

fn file_response(
    status: u16,
    content_type: &'static str,
    path: Utf8PathBuf,
    size_bytes: u64,
) -> PeerResponse {
    let header = response_header(status, content_type, size_bytes);
    PeerResponse::File { header, path }
}

fn binary_response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let header = response_header(status, content_type, body.len() as u64);
    let mut response = header.into_bytes();
    response.extend_from_slice(body);
    response
}

fn response_header(status: u16, content_type: &str, content_length: u64) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {content_length}\r\nconnection: close\r\n\r\n"
    )
}

fn error_response(error: &PeerServerError) -> PeerResponse {
    let status = match error {
        PeerServerError::Unauthorized
        | PeerServerError::PeerStore(_)
        | PeerServerError::Signing(_) => 401,
        PeerServerError::NotFound => 404,
        PeerServerError::Conflict => 409,
        PeerServerError::RequestTooLarge => 413,
        PeerServerError::RequestTimeout => 408,
        PeerServerError::InvalidRequest
        | PeerServerError::HashMismatch
        | PeerServerError::FolderLimitExceeded { .. }
        | PeerServerError::Json(_) => 400,
        PeerServerError::Io(_) | PeerServerError::Core(_) | PeerServerError::Filesystem { .. } => {
            500
        }
    };
    json_response(
        status,
        &ErrorResponse {
            error: error.public_code(),
        },
    )
    .unwrap_or_else(|_| {
        PeerResponse::Bytes(
            "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .as_bytes()
                .to_vec(),
        )
    })
}

async fn write_response(
    stream: &mut TcpStream,
    response: PeerResponse,
) -> Result<(), PeerServerError> {
    match response {
        PeerResponse::Bytes(bytes) => stream.write_all(&bytes).await?,
        PeerResponse::File { header, path } => {
            stream.write_all(header.as_bytes()).await?;
            let mut file = tokio::fs::File::open(&path).await.map_err(|source| {
                PeerServerError::Filesystem {
                    path: path.clone(),
                    source,
                }
            })?;
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let read =
                    file.read(&mut buffer)
                        .await
                        .map_err(|source| PeerServerError::Filesystem {
                            path: path.clone(),
                            source,
                        })?;
                if read == 0 {
                    break;
                }
                stream.write_all(&buffer[..read]).await?;
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body_sha256_hex: String,
    body: HttpBody,
}

impl HttpRequest {
    fn memory_body(&self) -> Result<&[u8], PeerServerError> {
        match &self.body {
            HttpBody::Memory(bytes) => Ok(bytes),
            HttpBody::File(_) => Err(PeerServerError::InvalidRequest),
        }
    }

    fn file_body(&self) -> Result<FileBody, PeerServerError> {
        match &self.body {
            HttpBody::Memory(_) => Err(PeerServerError::InvalidRequest),
            HttpBody::File(body) => Ok(body.clone()),
        }
    }
}

#[derive(Clone, Debug)]
enum HttpBody {
    Memory(Vec<u8>),
    File(FileBody),
}

impl HttpBody {
    fn sha256_hex(&self) -> String {
        match self {
            Self::Memory(bytes) => {
                let digest = Sha256::digest(bytes);
                hex::encode(digest)
            }
            Self::File(body) => body.sha256_hex.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct FileBody {
    path: Utf8PathBuf,
    size_bytes: u64,
    sha256_hex: String,
}

#[derive(Debug)]
enum PeerResponse {
    Bytes(Vec<u8>),
    File { header: String, path: Utf8PathBuf },
}

#[derive(Debug)]
struct SharedFile {
    path: Utf8PathBuf,
    size_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PresenceResponse {
    pub accepted: bool,
    pub received_from: String,
    pub local_presence: PeerPresence,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    device_id: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize)]
pub struct UploadFileResponse {
    pub accepted: bool,
    pub path: RelativePath,
    pub size_bytes: u64,
    pub content_hash: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerServerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("filesystem error at {path}: {source}")]
    Filesystem {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("core error: {0}")]
    Core(#[from] syncer_core::CoreError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid HTTP request")]
    InvalidRequest,
    #[error("request too large")]
    RequestTooLarge,
    #[error("request timed out")]
    RequestTimeout,
    #[error("not found")]
    NotFound,
    #[error("target already exists")]
    Conflict,
    #[error("content hash does not match body")]
    HashMismatch,
    #[error("folder size limit exceeded: requested {requested}, remaining {remaining}")]
    FolderLimitExceeded { remaining: u64, requested: u64 },
    #[error("unauthorized peer request")]
    Unauthorized,
    #[error("{0}")]
    PeerStore(#[from] crate::peer_store::PeerStoreError),
    #[error("{0}")]
    Signing(#[from] crate::request_signing::SigningError),
}

impl PeerServerError {
    fn public_code(&self) -> String {
        match self {
            Self::InvalidRequest | Self::Json(_) => "invalid_request",
            Self::RequestTooLarge => "request_too_large",
            Self::RequestTimeout => "request_timeout",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::HashMismatch => "hash_mismatch",
            Self::FolderLimitExceeded { .. } => "folder_limit_exceeded",
            Self::Unauthorized | Self::PeerStore(_) | Self::Signing(_) => "unauthorized",
            Self::Io(_) | Self::Core(_) | Self::Filesystem { .. } => "internal_error",
        }
        .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use syncer_core::{
        ContentHash, FolderMode, FolderSizeLimit, LinuxFolderScanner, RelativePath, StateDatabase,
    };

    use crate::profile::LocalDeviceProfile;

    use super::{FileBody, PeerServerError, shared_file, write_uploaded_file};

    #[tokio::test]
    async fn reads_only_indexed_shared_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("syncer-peer-file-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(root.join("docs"))?;
        fs::write(root.join("docs/readme.txt"), b"hello peer")?;

        let folder = syncer_core::FolderStore::from_std_path(&root)?;
        folder
            .initialize(
                "Peer File Test".to_owned(),
                FolderMode::Bidirectional,
                FolderSizeLimit::new(1024 * 1024, 80)?,
                Vec::new(),
            )
            .await?;
        let profile = LocalDeviceProfile::create("peer".to_owned())?;
        let database = syncer_core::StateDatabase::open(&folder.state_db_path()).await?;
        let scanner = LinuxFolderScanner::from_std_path(&root, profile.device_id)?;
        scanner.scan_into(&database).await?;

        let file = shared_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("docs/readme.txt")?,
        )
        .await?;
        let missing = shared_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("docs/missing.txt")?,
        )
        .await;

        assert_eq!(fs::read(file.path)?, b"hello peer");
        assert_eq!(file.size_bytes, 10);
        assert!(missing.is_err());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[tokio::test]
    async fn writes_uploaded_file_and_rejects_existing_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("syncer-peer-upload-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root)?;

        let folder = syncer_core::FolderStore::from_std_path(&root)?;
        folder
            .initialize(
                "Peer Upload Test".to_owned(),
                FolderMode::Bidirectional,
                FolderSizeLimit::new(1024 * 1024, 80)?,
                Vec::new(),
            )
            .await?;
        let profile = LocalDeviceProfile::create("peer".to_owned())?;
        let body = b"uploaded body";
        let hash = ContentHash::from_bytes(body);
        let first_body_path = folder.syncer_dir().join("first-upload.body");
        let second_body_path = folder.syncer_dir().join("second-upload.body");
        fs::write(&first_body_path, body)?;
        fs::write(&second_body_path, body)?;

        let response = write_uploaded_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("incoming/file.txt")?,
            body.len() as u64,
            hash.as_hex(),
            FileBody {
                path: first_body_path,
                size_bytes: body.len() as u64,
                sha256_hex: String::new(),
            },
        )
        .await?;
        let duplicate = write_uploaded_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("incoming/file.txt")?,
            body.len() as u64,
            hash.as_hex(),
            FileBody {
                path: second_body_path,
                size_bytes: body.len() as u64,
                sha256_hex: String::new(),
            },
        )
        .await;
        let database = StateDatabase::open(&folder.state_db_path()).await?;
        let files = database.list_files().await?;

        assert!(response.accepted);
        assert_eq!(fs::read(root.join("incoming/file.txt"))?, body);
        assert_eq!(files.len(), 1);
        assert!(matches!(duplicate, Err(PeerServerError::Conflict)));
        fs::remove_dir_all(root).ok();
        Ok(())
    }
}
