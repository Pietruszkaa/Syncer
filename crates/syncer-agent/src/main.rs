mod profile;

use std::fs::OpenOptions;
use std::io::Write;

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use serde::Serialize;
use syncer_core::{
    DEFAULT_FOLDER_SIZE_LIMIT_BYTES, FolderMode, FolderSizeLimit, FolderStore, LinuxFolderScanner,
    PendingTransfer, RelativePath, StateDatabase,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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

    let serialized = serde_json::to_vec_pretty(&profile)?;
    file.write_all(&serialized)
        .map_err(|source| AgentError::WriteConfig {
            path: config.clone(),
            source,
        })?;
    file.write_all(b"\n")
        .map_err(|source| AgentError::WriteConfig {
            path: config.clone(),
            source,
        })?;

    print_json(&InitDeviceOutput {
        device_id: profile.device_id.to_string(),
        device_name: profile.device_name.as_str().to_owned(),
        config_path: config,
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
    config_path: Utf8PathBuf,
}

#[derive(Debug, Serialize)]
struct InitFolderOutput {
    folder_id: String,
    folder_path: Utf8PathBuf,
    config_path: Utf8PathBuf,
    state_db_path: Utf8PathBuf,
}
