mod path_codec;
mod peer_client;
mod peer_server;
mod peer_store;
mod profile;
mod request_signing;

use std::fs::OpenOptions;
use std::io::Write;

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use serde::Serialize;
use syncer_core::{
    ContentHash, DEFAULT_FOLDER_SIZE_LIMIT_BYTES, FileEntry, FileKind, FileVersion, FolderManifest,
    FolderMode, FolderSizeLimit, FolderStore, LinuxFolderScanner, PeerPresence, PendingTransfer,
    RelativePath, StateDatabase, SyncOperationRecord, plan_manifest_sync_for_mode,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::peer_server::PeerServerConfig;
use crate::peer_store::{PairingTicketDocument, PeerStoreDocument};
use crate::profile::LocalDeviceProfile;

#[derive(Debug, Parser)]
#[command(name = "syncer-agent")]
#[command(about = "Local Syncer agent control utility")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    InitDevice {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = ".syncer-local/device.json")]
        config: Utf8PathBuf,
    },
    InitFolder {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long)]
        display_name: String,
        #[arg(long, default_value_t = DEFAULT_FOLDER_SIZE_LIMIT_BYTES)]
        max_bytes: u64,
        #[arg(long, default_value_t = 80)]
        warning_threshold_percent: u8,
    },
    ScanFolder {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    FolderStatus {
        #[arg(long)]
        path: Utf8PathBuf,
    },
    ExportManifest {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    Serve {
        #[arg(long, default_value = "127.0.0.1:57421")]
        bind: String,
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
        #[arg(long, default_value = ".syncer-local/peers.json")]
        peers: Utf8PathBuf,
    },
    PairingTicket {
        #[arg(long)]
        endpoint: String,
        #[arg(long, default_value_t = 300)]
        ttl_seconds: i64,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
        #[arg(long)]
        out: Option<Utf8PathBuf>,
    },
    AcceptPeer {
        #[arg(long)]
        ticket: Utf8PathBuf,
        #[arg(long, default_value = ".syncer-local/peers.json")]
        peers: Utf8PathBuf,
    },
    PingPeer {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        shared_secret: String,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    FetchManifest {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        shared_secret: String,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    PlanSync {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        shared_secret: String,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    EnqueueSyncPlan {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        shared_secret: String,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    QueueStatus {
        #[arg(long)]
        path: Utf8PathBuf,
    },
    RetryFailed {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long)]
        remote_device_id: Option<String>,
    },
    ExecuteSyncQueue {
        #[arg(long)]
        path: Utf8PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        shared_secret: String,
        #[arg(long, default_value_t = 100)]
        limit: u64,
        #[arg(long, default_value = ".syncer-local/device.json")]
        device_config: Utf8PathBuf,
    },
    PlanFolderLimit {
        #[arg(long)]
        max_bytes: u64,
        #[arg(long, default_value_t = 80)]
        warning_threshold_percent: u8,
        #[arg(long, default_value_t = 0)]
        used_bytes: u64,
        #[arg(long = "file", value_parser = parse_pending_transfer)]
        files: Vec<PendingTransferArg>,
    },
}

#[derive(Clone, Debug)]
struct PendingTransferArg {
    path: String,
    size_bytes: u64,
    queued_at: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
enum AgentError {
    #[error("{0}")]
    Core(#[from] syncer_core::CoreError),
    #[error("failed to parse datetime as RFC3339")]
    DateTime(#[from] time::error::Parse),
    #[error("file argument must use path:size_bytes:queued_at_rfc3339")]
    InvalidPendingTransfer,
    #[error("failed to create parent directory {path}: {source}")]
    CreateDirectory {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create config file {path}: {source}")]
    CreateConfig {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write config file {path}: {source}")]
    WriteConfig {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("failed to serialize JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    ProfileRead(#[from] profile::ProfileReadError),
    #[error("{0}")]
    PeerServer(#[from] peer_server::PeerServerError),
    #[error("{0}")]
    PeerClient(#[from] peer_client::PeerClientError),
    #[error("{0}")]
    PeerStore(#[from] peer_store::PeerStoreError),
    #[error("{0}")]
    Transfer(#[from] TransferError),
}

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let cli = Cli::parse();

    match cli.command {
        Command::InitDevice { name, config } => init_device(name, config),
        Command::InitFolder {
            path,
            display_name,
            max_bytes,
            warning_threshold_percent,
        } => init_folder(path, display_name, max_bytes, warning_threshold_percent).await,
        Command::ScanFolder {
            path,
            device_config,
        } => scan_folder(path, device_config).await,
        Command::FolderStatus { path } => folder_status(path).await,
        Command::ExportManifest {
            path,
            device_config,
        } => export_manifest(path, device_config).await,
        Command::Serve {
            bind,
            path,
            device_config,
            peers,
        } => serve(bind, path, device_config, peers).await,
        Command::PairingTicket {
            endpoint,
            ttl_seconds,
            device_config,
            out,
        } => pairing_ticket(endpoint, ttl_seconds, &device_config, out),
        Command::AcceptPeer { ticket, peers } => accept_peer(&ticket, peers),
        Command::PingPeer {
            endpoint,
            shared_secret,
            device_config,
        } => ping_peer(&endpoint, &shared_secret, &device_config),
        Command::FetchManifest {
            endpoint,
            shared_secret,
            device_config,
        } => fetch_manifest(&endpoint, &shared_secret, &device_config),
        Command::PlanSync {
            path,
            endpoint,
            shared_secret,
            device_config,
        } => plan_sync(path, &endpoint, &shared_secret, &device_config).await,
        Command::EnqueueSyncPlan {
            path,
            endpoint,
            shared_secret,
            device_config,
        } => enqueue_sync_plan(path, &endpoint, &shared_secret, &device_config).await,
        Command::QueueStatus { path } => queue_status(path).await,
        Command::RetryFailed {
            path,
            remote_device_id,
        } => retry_failed(path, remote_device_id.as_deref()).await,
        Command::ExecuteSyncQueue {
            path,
            endpoint,
            shared_secret,
            limit,
            device_config,
        } => execute_sync_queue(path, &endpoint, &shared_secret, limit, &device_config).await,
        Command::PlanFolderLimit {
            max_bytes,
            warning_threshold_percent,
            used_bytes,
            files,
        } => plan_folder_limit(max_bytes, warning_threshold_percent, used_bytes, files),
    }
}

async fn init_folder(
    path: Utf8PathBuf,
    display_name: String,
    max_bytes: u64,
    warning_threshold_percent: u8,
) -> Result<(), AgentError> {
    let limit = FolderSizeLimit::new(max_bytes, warning_threshold_percent)?;
    let store = FolderStore::new(path.clone());
    let document = store
        .initialize(display_name, FolderMode::Bidirectional, limit, Vec::new())
        .await?;
    StateDatabase::open(&store.state_db_path()).await?;

    print_json(&InitFolderOutput {
        folder_id: document.folder.id.to_string(),
        folder_path: path,
        config_path: store.folder_json_path(),
        state_db_path: store.state_db_path(),
    })
}

async fn scan_folder(path: Utf8PathBuf, device_config: Utf8PathBuf) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(&device_config)?;
    let store = FolderStore::new(path.clone());
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let scanner = LinuxFolderScanner::new(path, profile.device_id);
    let summary = scanner.scan_into(&database).await?;

    print_json(&summary)
}

async fn folder_status(path: Utf8PathBuf) -> Result<(), AgentError> {
    let store = FolderStore::new(path);
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let status = database.folder_status().await?;

    print_json(&status)
}

async fn export_manifest(path: Utf8PathBuf, device_config: Utf8PathBuf) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(&device_config)?;
    let manifest = load_manifest(path, &profile).await?;

    print_json(&manifest)
}

async fn serve(
    bind: String,
    path: Utf8PathBuf,
    device_config: Utf8PathBuf,
    peers: Utf8PathBuf,
) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(&device_config)?;
    peer_server::run(PeerServerConfig {
        bind,
        folder_path: path,
        peers_path: peers,
        profile,
    })
    .await?;
    Ok(())
}

fn pairing_ticket(
    endpoint: String,
    ttl_seconds: i64,
    device_config: &Utf8PathBuf,
    out: Option<Utf8PathBuf>,
) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(device_config)?;
    let ticket = PairingTicketDocument::create(&profile, endpoint, ttl_seconds)?;

    if let Some(path) = out {
        write_json_file(&path, &ticket)?;
        print_json(&PairingTicketOutput {
            ticket_path: path,
            device_id: ticket.device_id.to_string(),
            expires_at: ticket.expires_at,
        })
    } else {
        print_json(&ticket)
    }
}

fn accept_peer(ticket: &Utf8PathBuf, peers: Utf8PathBuf) -> Result<(), AgentError> {
    let ticket = PairingTicketDocument::read(ticket)?;
    let mut store = PeerStoreDocument::read_or_default(&peers)?;
    let device_id = ticket.device_id.to_string();
    let device_name = ticket.device_name.clone();
    store.accept_ticket(ticket)?;
    store.write(&peers)?;

    print_json(&AcceptPeerOutput {
        accepted: true,
        device_id,
        device_name,
        peers_path: peers,
    })
}

fn ping_peer(
    endpoint: &str,
    shared_secret: &str,
    device_config: &Utf8PathBuf,
) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(device_config)?;
    let response = peer_client::post_presence(
        endpoint,
        &profile.device_id.to_string(),
        shared_secret,
        &PeerPresence {
            device_id: profile.device_id,
            sent_at: OffsetDateTime::now_utc(),
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    )?;

    print_json(&response)
}

fn fetch_manifest(
    endpoint: &str,
    shared_secret: &str,
    device_config: &Utf8PathBuf,
) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(device_config)?;
    let manifest =
        peer_client::fetch_manifest(endpoint, &profile.device_id.to_string(), shared_secret)?;

    print_json(&manifest)
}

async fn plan_sync(
    path: Utf8PathBuf,
    endpoint: &str,
    shared_secret: &str,
    device_config: &Utf8PathBuf,
) -> Result<(), AgentError> {
    let (plan, _) = build_sync_plan(path, endpoint, shared_secret, device_config).await?;

    print_json(&plan)
}

async fn enqueue_sync_plan(
    path: Utf8PathBuf,
    endpoint: &str,
    shared_secret: &str,
    device_config: &Utf8PathBuf,
) -> Result<(), AgentError> {
    let (plan, database) = build_sync_plan(path, endpoint, shared_secret, device_config).await?;
    let status = database.enqueue_sync_plan(&plan).await?;

    print_json(&EnqueueSyncPlanOutput {
        queued: status,
        plan_summary: plan.summary,
    })
}

async fn queue_status(path: Utf8PathBuf) -> Result<(), AgentError> {
    let store = FolderStore::new(path);
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let status = database.queue_status().await?;
    let operations = database.list_sync_operations().await?;

    print_json(&QueueStatusOutput { status, operations })
}

async fn retry_failed(path: Utf8PathBuf, remote_device_id: Option<&str>) -> Result<(), AgentError> {
    let store = FolderStore::new(path);
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let summary = database
        .retry_failed_sync_operations(remote_device_id)
        .await?;

    print_json(&summary)
}

async fn execute_sync_queue(
    path: Utf8PathBuf,
    endpoint: &str,
    shared_secret: &str,
    limit: u64,
    device_config: &Utf8PathBuf,
) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::read(device_config)?;
    let store = FolderStore::new(path.clone());
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let scanner = LinuxFolderScanner::new(path, profile.device_id);
    scanner.scan_into(&database).await?;
    let remote_manifest =
        peer_client::fetch_manifest(endpoint, &profile.device_id.to_string(), shared_secret)?;
    let remote_device_id = remote_manifest.device_id.to_string();
    let download_operations = database
        .list_pending_download_operations(&remote_device_id, limit)
        .await?;
    let remaining_limit = limit.saturating_sub(download_operations.len() as u64);
    let upload_operations = database
        .list_pending_upload_operations(&remote_device_id, remaining_limit)
        .await?;
    let mut results = Vec::with_capacity(download_operations.len() + upload_operations.len());
    let mut completed = 0_u64;
    let mut failed = 0_u64;

    for operation in download_operations {
        database.mark_sync_operation_started(&operation.id).await?;
        match download_operation(
            &store,
            &database,
            &profile,
            endpoint,
            shared_secret,
            &remote_manifest,
            &operation,
        )
        .await
        {
            Ok(result) => {
                completed = completed.saturating_add(1);
                results.push(result);
            }
            Err(error) => {
                failed = failed.saturating_add(1);
                database
                    .mark_sync_operation_failed(&operation.id, &error.to_string())
                    .await?;
                results.push(TransferOperationResult {
                    operation_id: operation.id,
                    path: operation.path,
                    status: "failed",
                    size_bytes: operation.remote_size_bytes.unwrap_or_default(),
                    error: Some(error.to_string()),
                });
            }
        }
    }
    for operation in upload_operations {
        database.mark_sync_operation_started(&operation.id).await?;
        match upload_operation(
            &store,
            &database,
            &profile,
            endpoint,
            shared_secret,
            &remote_manifest,
            &operation,
        )
        .await
        {
            Ok(result) => {
                completed = completed.saturating_add(1);
                results.push(result);
            }
            Err(error) => {
                failed = failed.saturating_add(1);
                database
                    .mark_sync_operation_failed(&operation.id, &error.to_string())
                    .await?;
                results.push(TransferOperationResult {
                    operation_id: operation.id,
                    path: operation.path,
                    status: "failed",
                    size_bytes: operation.local_size_bytes.unwrap_or_default(),
                    error: Some(error.to_string()),
                });
            }
        }
    }

    print_json(&ExecuteSyncQueueOutput {
        completed,
        failed,
        results,
    })
}

async fn upload_operation(
    store: &FolderStore,
    database: &StateDatabase,
    profile: &LocalDeviceProfile,
    endpoint: &str,
    shared_secret: &str,
    remote_manifest: &FolderManifest,
    operation: &SyncOperationRecord,
) -> Result<TransferOperationResult, TransferError> {
    if remote_manifest.files.iter().any(|file| {
        file.path == operation.path && file.sync_state == syncer_core::FileSyncState::LocalAvailable
    }) {
        return Err(TransferError::RemoteTargetExists);
    }
    let local_file = database
        .list_files()
        .await?
        .into_iter()
        .find(|file| file.path == operation.path)
        .ok_or(TransferError::LocalFileMissing)?;
    if local_file.kind != FileKind::File {
        return Err(TransferError::LocalPathIsNotFile);
    }
    let expected_hash = operation
        .local_content_hash
        .as_deref()
        .or(local_file.content_hash.as_deref())
        .ok_or(TransferError::LocalHashMissing)?;
    if local_file.content_hash.as_deref() != Some(expected_hash) {
        return Err(TransferError::LocalFileChanged);
    }
    if operation
        .local_size_bytes
        .is_some_and(|size_bytes| size_bytes != local_file.size_bytes)
    {
        return Err(TransferError::LocalFileChanged);
    }

    let target = store.root().join(operation.path.as_path());
    let response = peer_client::put_file_from_path(
        endpoint,
        &profile.device_id.to_string(),
        shared_secret,
        &operation.path,
        expected_hash,
        &target,
        local_file.size_bytes,
    )?;
    if !response.accepted
        || response.path != operation.path
        || response.size_bytes != local_file.size_bytes
        || response.content_hash != expected_hash
    {
        return Err(TransferError::UploadRejected);
    }

    database.mark_sync_operation_done(&operation.id).await?;
    Ok(TransferOperationResult {
        operation_id: operation.id.clone(),
        path: operation.path.clone(),
        status: "done",
        size_bytes: local_file.size_bytes,
        error: None,
    })
}

async fn build_sync_plan(
    path: Utf8PathBuf,
    endpoint: &str,
    shared_secret: &str,
    device_config: &Utf8PathBuf,
) -> Result<(syncer_core::SyncPlan, StateDatabase), AgentError> {
    let profile = LocalDeviceProfile::read(device_config)?;
    let store = FolderStore::new(path.clone());
    let database = StateDatabase::open(&store.state_db_path()).await?;
    let scanner = LinuxFolderScanner::new(path.clone(), profile.device_id);
    scanner.scan_into(&database).await?;

    let folder = store.read().await?;
    let local_manifest = database
        .export_manifest(folder.folder.id, profile.device_id)
        .await?;
    let remote_manifest =
        peer_client::fetch_manifest(endpoint, &profile.device_id.to_string(), shared_secret)?;
    let plan = plan_manifest_sync_for_mode(&local_manifest, &remote_manifest, folder.folder.mode);

    Ok((plan, database))
}

async fn download_operation(
    store: &FolderStore,
    database: &StateDatabase,
    profile: &LocalDeviceProfile,
    endpoint: &str,
    shared_secret: &str,
    remote_manifest: &FolderManifest,
    operation: &SyncOperationRecord,
) -> Result<TransferOperationResult, TransferError> {
    let remote_file = remote_manifest
        .files
        .iter()
        .find(|file| file.path == operation.path)
        .ok_or(TransferError::RemoteFileMissing)?;
    if remote_file.kind != FileKind::File {
        return Err(TransferError::RemotePathIsNotFile);
    }
    let expected_hash = operation
        .remote_content_hash
        .as_deref()
        .or(remote_file.content_hash.as_deref())
        .ok_or(TransferError::RemoteHashMissing)?;
    if remote_file.content_hash.as_deref() != Some(expected_hash) {
        return Err(TransferError::RemoteFileChanged);
    }
    if operation
        .remote_size_bytes
        .is_some_and(|size_bytes| size_bytes != remote_file.size_bytes)
    {
        return Err(TransferError::RemoteFileChanged);
    }
    enforce_local_download_limit(store, database, &operation.path, remote_file.size_bytes).await?;

    let target = store.root().join(operation.path.as_path());
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|source| TransferError::Filesystem {
            path: target.clone(),
            source,
        })?
    {
        return Err(TransferError::LocalTargetExists);
    }

    let temporary = download_temporary_path(store, &operation.id).await?;
    let transferred = peer_client::fetch_file_to_path(
        endpoint,
        &profile.device_id.to_string(),
        shared_secret,
        &operation.path,
        remote_file.size_bytes,
        &temporary,
    )?;
    if transferred.size_bytes != remote_file.size_bytes {
        return Err(TransferError::SizeMismatch {
            expected: remote_file.size_bytes,
            actual: transferred.size_bytes,
        });
    }
    if transferred.content_hash != expected_hash {
        return Err(TransferError::HashMismatch);
    }

    publish_download(store, &operation.path, &temporary).await?;
    database
        .upsert_file(&FileEntry {
            path: operation.path.clone(),
            kind: FileKind::File,
            size_bytes: transferred.size_bytes,
            modified_at: OffsetDateTime::now_utc(),
            content_hash: Some(ContentHash::from_hex(transferred.content_hash.clone())),
            block_hashes: Vec::new(),
            version: FileVersion {
                generation: remote_file.generation,
                device_id: profile.device_id,
            },
        })
        .await?;
    database.mark_sync_operation_done(&operation.id).await?;

    Ok(TransferOperationResult {
        operation_id: operation.id.clone(),
        path: operation.path.clone(),
        status: "done",
        size_bytes: transferred.size_bytes,
        error: None,
    })
}

async fn enforce_local_download_limit(
    store: &FolderStore,
    database: &StateDatabase,
    relative_path: &RelativePath,
    size_bytes: u64,
) -> Result<(), TransferError> {
    let folder = store.read().await?;
    let status = database.folder_status().await?;
    let remaining = folder
        .folder
        .size_limit
        .remaining_bytes(status.indexed_bytes);
    if size_bytes > remaining {
        database
            .record_skipped_folder_limit(&PendingTransfer {
                operation_id: syncer_core::OperationId::new(),
                path: relative_path.clone(),
                size_bytes,
                queued_at: OffsetDateTime::now_utc(),
            })
            .await?;
        return Err(TransferError::FolderLimitExceeded {
            remaining,
            requested: size_bytes,
        });
    }
    Ok(())
}

async fn download_temporary_path(
    store: &FolderStore,
    operation_id: &str,
) -> Result<Utf8PathBuf, TransferError> {
    let temporary_dir = store.syncer_dir().join("tmp").join("downloads");
    tokio::fs::create_dir_all(&temporary_dir)
        .await
        .map_err(|source| TransferError::Filesystem {
            path: temporary_dir.clone(),
            source,
        })?;
    Ok(temporary_dir.join(format!("{operation_id}.part")))
}

async fn publish_download(
    store: &FolderStore,
    relative_path: &RelativePath,
    temporary: &camino::Utf8Path,
) -> Result<(), TransferError> {
    let target = store.root().join(relative_path.as_path());
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| TransferError::Filesystem {
                path: parent.to_owned(),
                source,
            })?;
    }
    tokio::fs::rename(temporary, &target)
        .await
        .map_err(|source| TransferError::Filesystem {
            path: target,
            source,
        })?;

    Ok(())
}

async fn load_manifest(
    path: Utf8PathBuf,
    profile: &LocalDeviceProfile,
) -> Result<FolderManifest, AgentError> {
    let store = FolderStore::new(path);
    let folder = store.read().await?;
    let database = StateDatabase::open(&store.state_db_path()).await?;

    Ok(database
        .export_manifest(folder.folder.id, profile.device_id)
        .await?)
}

fn init_device(name: String, config: Utf8PathBuf) -> Result<(), AgentError> {
    let profile = LocalDeviceProfile::create(name)?;

    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent).map_err(|source| AgentError::CreateDirectory {
            path: parent.to_owned(),
            source,
        })?;
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config)
        .map_err(|source| AgentError::CreateConfig {
            path: config.clone(),
            source,
        })?;

    write_json_to_writer(&mut file, &profile, &config)?;

    print_json(&InitDeviceOutput {
        device_id: profile.device_id.to_string(),
        device_name: profile.device_name.as_str().to_owned(),
        identity_public_hex: profile.identity_public_hex,
        config_path: config,
    })
}

fn write_json_file(path: &Utf8PathBuf, value: &impl Serialize) -> Result<(), AgentError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| AgentError::CreateDirectory {
            path: parent.to_owned(),
            source,
        })?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| AgentError::CreateConfig {
            path: path.clone(),
            source,
        })?;
    write_json_to_writer(&mut file, value, path)
}

fn write_json_to_writer(
    writer: &mut impl Write,
    value: &impl Serialize,
    path: &Utf8PathBuf,
) -> Result<(), AgentError> {
    let serialized = serde_json::to_vec_pretty(value)?;
    writer
        .write_all(&serialized)
        .map_err(|source| AgentError::WriteConfig {
            path: path.clone(),
            source,
        })?;
    writer
        .write_all(b"\n")
        .map_err(|source| AgentError::WriteConfig {
            path: path.clone(),
            source,
        })
}

fn plan_folder_limit(
    max_bytes: u64,
    warning_threshold_percent: u8,
    used_bytes: u64,
    files: Vec<PendingTransferArg>,
) -> Result<(), AgentError> {
    let limit = FolderSizeLimit::new(max_bytes, warning_threshold_percent)?;
    let pending = files
        .into_iter()
        .map(|file| {
            Ok(PendingTransfer {
                operation_id: syncer_core::OperationId::new(),
                path: RelativePath::parse(file.path)?,
                size_bytes: file.size_bytes,
                queued_at: file.queued_at,
            })
        })
        .collect::<Result<Vec<_>, syncer_core::CoreError>>()?;

    let plan = limit.plan_transfers(used_bytes, pending);
    print_json(&plan)
}

fn parse_pending_transfer(value: &str) -> Result<PendingTransferArg, AgentError> {
    let mut parts = value.splitn(3, ':');
    let path = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or(AgentError::InvalidPendingTransfer)?
        .to_owned();
    let size_bytes = parts
        .next()
        .ok_or(AgentError::InvalidPendingTransfer)?
        .parse()
        .map_err(|_| AgentError::InvalidPendingTransfer)?;
    let queued_at = OffsetDateTime::parse(
        parts.next().ok_or(AgentError::InvalidPendingTransfer)?,
        &Rfc3339,
    )?;

    Ok(PendingTransferArg {
        path,
        size_bytes,
        queued_at,
    })
}

fn print_json(value: &impl Serialize) -> Result<(), AgentError> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, value)?;
    handle
        .write_all(b"\n")
        .map_err(|source| AgentError::WriteConfig {
            path: Utf8PathBuf::from("<stdout>"),
            source,
        })
}

#[derive(Debug, Serialize)]
struct InitDeviceOutput {
    device_id: String,
    device_name: String,
    identity_public_hex: String,
    config_path: Utf8PathBuf,
}

#[derive(Debug, Serialize)]
struct InitFolderOutput {
    folder_id: String,
    folder_path: Utf8PathBuf,
    config_path: Utf8PathBuf,
    state_db_path: Utf8PathBuf,
}

#[derive(Debug, Serialize)]
struct PairingTicketOutput {
    ticket_path: Utf8PathBuf,
    device_id: String,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct AcceptPeerOutput {
    accepted: bool,
    device_id: String,
    device_name: String,
    peers_path: Utf8PathBuf,
}

#[derive(Debug, Serialize)]
struct EnqueueSyncPlanOutput {
    queued: syncer_core::QueueStatus,
    plan_summary: syncer_core::SyncPlanSummary,
}

#[derive(Debug, Serialize)]
struct QueueStatusOutput {
    status: syncer_core::QueueStatus,
    operations: Vec<syncer_core::SyncOperationRecord>,
}

#[derive(Debug, Serialize)]
struct ExecuteSyncQueueOutput {
    completed: u64,
    failed: u64,
    results: Vec<TransferOperationResult>,
}

#[derive(Debug, Serialize)]
struct TransferOperationResult {
    operation_id: String,
    path: RelativePath,
    status: &'static str,
    size_bytes: u64,
    error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum TransferError {
    #[error("local file is missing")]
    LocalFileMissing,
    #[error("local path is not a regular file")]
    LocalPathIsNotFile,
    #[error("local file hash is missing")]
    LocalHashMissing,
    #[error("local file changed after this operation was queued")]
    LocalFileChanged,
    #[error("remote file is missing")]
    RemoteFileMissing,
    #[error("remote path is not a regular file")]
    RemotePathIsNotFile,
    #[error("remote file hash is missing")]
    RemoteHashMissing,
    #[error("remote file changed after this operation was queued")]
    RemoteFileChanged,
    #[error("remote target already exists")]
    RemoteTargetExists,
    #[error("peer rejected uploaded file")]
    UploadRejected,
    #[error("local target already exists")]
    LocalTargetExists,
    #[error("downloaded size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("downloaded file hash does not match remote manifest")]
    HashMismatch,
    #[error("folder size limit exceeded: requested {requested}, remaining {remaining}")]
    FolderLimitExceeded { remaining: u64, requested: u64 },
    #[error("filesystem error at {path}: {source}")]
    Filesystem {
        path: Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    PeerClient(#[from] peer_client::PeerClientError),
    #[error("{0}")]
    Core(#[from] syncer_core::CoreError),
}
