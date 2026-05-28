use std::collections::HashMap;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
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
use crate::request_signing::verify_request_signature;

const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
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
    let response = match read_request(&mut stream).await {
        Ok(request) => route_request(&request, &config)
            .await
            .unwrap_or_else(|error| error_response(&error)),
        Err(error) => error_response(&error),
    };
    stream.write_all(&response).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn route_request(
    request: &HttpRequest,
    config: &PeerServerConfig,
) -> Result<Vec<u8>, PeerServerError> {
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
            let presence: PeerPresence = serde_json::from_slice(&request.body)?;
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
            let bytes =
                read_shared_file(&config.folder_path, &config.profile, &relative_path).await?;
            Ok(binary_response(200, "application/octet-stream", &bytes))
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
                &request.body,
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

async fn read_shared_file(
    folder_path: &Utf8PathBuf,
    profile: &LocalDeviceProfile,
    relative_path: &syncer_core::RelativePath,
) -> Result<Vec<u8>, PeerServerError> {
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
    tokio::fs::read(&path)
        .await
        .map_err(|error| PeerServerError::Filesystem {
            path,
            source: error,
        })
}

async fn write_uploaded_file(
    folder_path: &Utf8PathBuf,
    profile: &LocalDeviceProfile,
    relative_path: &RelativePath,
    expected_size: u64,
    expected_hash: &str,
    bytes: &[u8],
) -> Result<UploadFileResponse, PeerServerError> {
    let actual_size = u64::try_from(bytes.len()).map_err(|_| PeerServerError::FileSizeOverflow)?;
    if actual_size != expected_size {
        return Err(PeerServerError::InvalidRequest);
    }
    let content_hash = ContentHash::from_bytes(bytes);
    if content_hash.as_hex() != expected_hash {
        return Err(PeerServerError::HashMismatch);
    }

    let store = FolderStore::new(folder_path.clone());
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

    publish_uploaded_file(&store, relative_path, bytes).await?;
    store.read().await?;
    let database = StateDatabase::open(&store.state_db_path()).await?;
    database
        .upsert_file(&FileEntry {
            path: relative_path.clone(),
            kind: FileKind::File,
            size_bytes: actual_size,
            modified_at: OffsetDateTime::now_utc(),
            content_hash: Some(content_hash),
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
        size_bytes: actual_size,
        content_hash: expected_hash.to_owned(),
    })
}

async fn publish_uploaded_file(
    store: &FolderStore,
    relative_path: &RelativePath,
    bytes: &[u8],
) -> Result<(), PeerServerError> {
    let temporary_dir = store.syncer_dir().join("tmp").join("uploads");
    tokio::fs::create_dir_all(&temporary_dir)
        .await
        .map_err(|source| PeerServerError::Filesystem {
            path: temporary_dir.clone(),
            source,
        })?;
    let temporary = temporary_dir.join(format!("{}.part", uuid::Uuid::now_v7()));
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|source| PeerServerError::Filesystem {
            path: temporary.clone(),
            source,
        })?;

    let target = store.root().join(relative_path.as_path());
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| PeerServerError::Filesystem {
                path: parent.to_owned(),
                source,
            })?;
    }
    tokio::fs::rename(&temporary, &target)
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

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, PeerServerError> {
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
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err(PeerServerError::RequestTooLarge);
        }
        if request_complete(&buffer)? {
            break;
        }
    }

    parse_request(&buffer)
}

fn request_complete(buffer: &[u8]) -> Result<bool, PeerServerError> {
    let Some(header_end) = find_header_end(buffer) else {
        return Ok(false);
    };
    let headers = parse_headers(&buffer[..header_end])?;
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    Ok(buffer.len() >= header_end + 4 + content_length)
}

fn parse_request(buffer: &[u8]) -> Result<HttpRequest, PeerServerError> {
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
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    let body_end = body_start.saturating_add(content_length);
    if body_end > buffer.len() {
        return Err(PeerServerError::InvalidRequest);
    }

    Ok(HttpRequest {
        method,
        path,
        headers,
        body: buffer[body_start..body_end].to_vec(),
    })
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
    verify_request_signature(
        signature,
        &request.method,
        &request.path,
        &device_id.to_string(),
        timestamp,
        &request.body,
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

fn json_response<T: Serialize>(status: u16, value: &T) -> Result<Vec<u8>, PeerServerError> {
    let body = serde_json::to_vec(value)?;
    Ok(binary_response(status, "application/json", &body))
}

fn binary_response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let mut response = header.into_bytes();
    response.extend_from_slice(body);
    response
}

fn error_response(error: &PeerServerError) -> Vec<u8> {
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
        | PeerServerError::FileSizeOverflow
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
        "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            .as_bytes()
            .to_vec()
    })
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
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
    #[error("file size exceeded supported range")]
    FileSizeOverflow,
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
            Self::FileSizeOverflow => "file_size_overflow",
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

    use super::{PeerServerError, read_shared_file, write_uploaded_file};

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

        let bytes = read_shared_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("docs/readme.txt")?,
        )
        .await?;
        let missing = read_shared_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("docs/missing.txt")?,
        )
        .await;

        assert_eq!(bytes, b"hello peer");
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

        let response = write_uploaded_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("incoming/file.txt")?,
            body.len() as u64,
            hash.as_hex(),
            body,
        )
        .await?;
        let duplicate = write_uploaded_file(
            &folder.root().to_owned(),
            &profile,
            &RelativePath::parse("incoming/file.txt")?,
            body.len() as u64,
            hash.as_hex(),
            body,
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
