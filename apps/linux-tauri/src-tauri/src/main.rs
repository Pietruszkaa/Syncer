mod agent_command;

use agent_command::{
    delete_file as build_delete_file, folder_status as build_folder_status,
    init_folder as build_init_folder, queue_status as build_queue_status,
    resolve_blocked as build_resolve_blocked, retry_failed as build_retry_failed,
    sync_once as build_sync_once, AgentCommandError, AgentRuntime, FolderConnection,
    PeerConnection,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};
use thiserror::Error;

const DEFAULT_FOLDER_SIZE_LIMIT_BYTES: u64 = 10_737_418_240;
const DEFAULT_SYNC_INTERVAL_SECONDS: u64 = 300;
const DEFAULT_WARNING_THRESHOLD_PERCENT: u8 = 80;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinuxState {
    selected_folder_id: Option<String>,
    folders: Vec<FolderConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderConfig {
    id: String,
    name: String,
    path: PathBuf,
    mode: String,
    max_bytes: u64,
    warning_threshold_percent: u8,
    sync_interval_seconds: u64,
    peer: Option<PeerConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddFolderInput {
    display_name: String,
    path: PathBuf,
    mode: String,
    max_bytes: Option<u64>,
    interval_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerConfig {
    device_id: String,
    endpoint: String,
    shared_secret: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardSnapshot {
    selected_folder_id: Option<String>,
    folders: Vec<FolderSnapshot>,
    peer: PeerSnapshot,
    queue: QueueSummary,
    operations: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct FolderSnapshot {
    config: FolderConfigSnapshot,
    status: FolderStatusSnapshot,
}

#[derive(Debug, Serialize)]
struct FolderConfigSnapshot {
    id: String,
    display_name: String,
    path: String,
    mode: String,
    interval_seconds: u64,
    max_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FolderStatusSnapshot {
    indexed_files: u64,
    indexed_bytes: u64,
    skipped_folder_limit: u64,
    unsynchronized_local_missing: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerSnapshot {
    online: bool,
    endpoint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueSummary {
    total: u64,
    pending: u64,
    blocked: u64,
    done: u64,
    failed: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueStatusOutput {
    status: QueueStatus,
    operations: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct QueueStatus {
    total: u64,
    pending: u64,
    blocked: u64,
    done: u64,
    failed: u64,
}

#[derive(Debug, Error)]
enum AppError {
    #[error("Linux state path is unavailable")]
    MissingStatePath,
    #[error("folder path must be an existing absolute directory")]
    InvalidFolderPath,
    #[error("folder name is required")]
    MissingFolderName,
    #[error("folder mode is not supported: {0}")]
    InvalidFolderMode(String),
    #[error("folder is already configured")]
    DuplicateFolder,
    #[error("folder is not configured: {0}")]
    MissingFolder(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Agent(#[from] AgentCommandError),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[tauri::command]
fn syncer_load_snapshot(app: AppHandle) -> Result<DashboardSnapshot, AppError> {
    let context = load_context(&app)?;
    load_snapshot(&context)
}

#[tauri::command]
fn syncer_add_folder(
    app: AppHandle,
    folder: AddFolderInput,
) -> Result<DashboardSnapshot, AppError> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::MissingStatePath)?;
    fs::create_dir_all(&app_data_dir)?;

    let runtime = AgentRuntime::from_env_or_default(&app_data_dir);
    let mut state = load_state(&app_data_dir.join("state.json"))?;
    let folder_config = create_folder_config(folder, &state)?;
    let connection = folder_config.connection();
    let init = build_init_folder(
        &runtime,
        &connection,
        &folder_config.name,
        folder_config.max_bytes,
        folder_config.warning_threshold_percent,
    );
    init.run_json::<serde_json::Value>()?;

    state.selected_folder_id = Some(folder_config.id.clone());
    state.folders.push(folder_config);
    write_state(&app_data_dir.join("state.json"), &state)?;

    load_snapshot(&AppContext { state, runtime })
}

#[tauri::command]
fn syncer_sync_once(app: AppHandle, folder_id: String) -> Result<DashboardSnapshot, AppError> {
    let context = load_context(&app)?;
    let folder = context.folder(&folder_id)?;
    let command = build_sync_once(&context.runtime, &folder.connection(), 100)?;
    command.run_json::<serde_json::Value>()?;
    let context = load_context(&app)?;
    load_snapshot(&context)
}

#[tauri::command]
fn syncer_retry_failed(app: AppHandle, folder_id: String) -> Result<DashboardSnapshot, AppError> {
    let context = load_context(&app)?;
    let folder = context.folder(&folder_id)?;
    let command = build_retry_failed(&context.runtime, &folder.connection());
    command.run_json::<serde_json::Value>()?;
    let context = load_context(&app)?;
    load_snapshot(&context)
}

#[tauri::command]
fn syncer_resolve_blocked(
    app: AppHandle,
    operation_id: String,
    action: String,
) -> Result<DashboardSnapshot, AppError> {
    let context = load_context(&app)?;
    let folder = context.selected_folder()?;
    let command = build_resolve_blocked(
        &context.runtime,
        &folder.connection(),
        &operation_id,
        &action,
    );
    command.run_json::<serde_json::Value>()?;
    let context = load_context(&app)?;
    load_snapshot(&context)
}

#[tauri::command]
fn syncer_delete_file(
    app: AppHandle,
    folder_id: String,
    path: String,
    scope: String,
) -> Result<DashboardSnapshot, AppError> {
    let context = load_context(&app)?;
    let folder = context.folder(&folder_id)?;
    let command = build_delete_file(&context.runtime, &folder.connection(), &path, &scope)?;
    command.run_json::<serde_json::Value>()?;
    let context = load_context(&app)?;
    load_snapshot(&context)
}

struct AppContext {
    state: LinuxState,
    runtime: AgentRuntime,
}

impl AppContext {
    fn selected_folder(&self) -> Result<&FolderConfig, AppError> {
        let folder_id = self
            .state
            .selected_folder_id
            .as_deref()
            .or_else(|| self.state.folders.first().map(|folder| folder.id.as_str()))
            .ok_or_else(|| AppError::MissingFolder("selected".to_owned()))?;

        self.folder(folder_id)
    }

    fn folder(&self, folder_id: &str) -> Result<&FolderConfig, AppError> {
        self.state
            .folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .ok_or_else(|| AppError::MissingFolder(folder_id.to_owned()))
    }
}

impl FolderConfig {
    fn connection(&self) -> FolderConnection {
        FolderConnection {
            path: self.path.clone(),
            peer: self.peer.as_ref().map(|peer| PeerConnection {
                endpoint: peer.endpoint.clone(),
                shared_secret: peer.shared_secret.clone(),
            }),
        }
    }
}

fn load_context(app: &AppHandle) -> Result<AppContext, AppError> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::MissingStatePath)?;
    fs::create_dir_all(&app_data_dir)?;

    Ok(AppContext {
        state: load_state(&app_data_dir.join("state.json"))?,
        runtime: AgentRuntime::from_env_or_default(&app_data_dir),
    })
}

fn load_state(path: &Path) -> Result<LinuxState, AppError> {
    if !path.exists() {
        return Ok(LinuxState {
            selected_folder_id: None,
            folders: Vec::new(),
        });
    }

    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn write_state(path: &Path, state: &LinuxState) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(state)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn create_folder_config(
    input: AddFolderInput,
    state: &LinuxState,
) -> Result<FolderConfig, AppError> {
    let name = input.display_name.trim();
    if name.is_empty() {
        return Err(AppError::MissingFolderName);
    }
    if !input.path.is_absolute() || !input.path.is_dir() {
        return Err(AppError::InvalidFolderPath);
    }
    if !matches!(
        input.mode.as_str(),
        "bidirectional" | "upload_only" | "download_only"
    ) {
        return Err(AppError::InvalidFolderMode(input.mode));
    }
    if state
        .folders
        .iter()
        .any(|folder| folder.path == input.path || folder.name == name)
    {
        return Err(AppError::DuplicateFolder);
    }

    Ok(FolderConfig {
        id: new_folder_id(name),
        name: name.to_owned(),
        path: input.path,
        mode: input.mode,
        max_bytes: input.max_bytes.unwrap_or(DEFAULT_FOLDER_SIZE_LIMIT_BYTES),
        warning_threshold_percent: DEFAULT_WARNING_THRESHOLD_PERCENT,
        sync_interval_seconds: input
            .interval_seconds
            .unwrap_or(DEFAULT_SYNC_INTERVAL_SECONDS)
            .max(30),
        peer: None,
    })
}

fn new_folder_id(name: &str) -> String {
    let slug = name
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else if character.is_whitespace() || character == '-' || character == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();

    format!(
        "{}-{suffix}",
        if slug.is_empty() { "folder" } else { &slug }
    )
}

fn load_snapshot(context: &AppContext) -> Result<DashboardSnapshot, AppError> {
    let selected_folder = context.selected_folder().ok();
    let queue_output = selected_folder.and_then(|folder| {
        build_queue_status(&context.runtime, &folder.connection())
            .run_json()
            .ok()
    });

    let peer = selected_folder
        .and_then(|folder| folder.peer.as_ref())
        .map(|peer| PeerSnapshot {
            online: false,
            endpoint: Some(peer.endpoint.clone()),
        })
        .unwrap_or(PeerSnapshot {
            online: false,
            endpoint: None,
        });

    let (queue, operations) = queue_output
        .map(|output: QueueStatusOutput| {
            (
                QueueSummary {
                    total: output.status.total,
                    pending: output.status.pending,
                    blocked: output.status.blocked,
                    done: output.status.done,
                    failed: output.status.failed,
                },
                output.operations,
            )
        })
        .unwrap_or((
            QueueSummary {
                total: 0,
                pending: 0,
                blocked: 0,
                done: 0,
                failed: 0,
            },
            Vec::new(),
        ));
    let folders = context
        .state
        .folders
        .iter()
        .map(|folder| folder_snapshot(&context.runtime, folder))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(DashboardSnapshot {
        selected_folder_id: context.state.selected_folder_id.clone(),
        folders,
        peer,
        queue,
        operations,
    })
}

fn folder_snapshot(
    runtime: &AgentRuntime,
    folder: &FolderConfig,
) -> Result<FolderSnapshot, AppError> {
    let status = build_folder_status(runtime, &folder.connection())
        .run_json()
        .unwrap_or_else(|_| FolderStatusSnapshot {
            indexed_files: 0,
            indexed_bytes: 0,
            skipped_folder_limit: 0,
            unsynchronized_local_missing: 0,
        });

    Ok(FolderSnapshot {
        config: FolderConfigSnapshot {
            id: folder.id.clone(),
            display_name: folder.name.clone(),
            path: folder.path.display().to_string(),
            mode: folder.mode.clone(),
            interval_seconds: folder.sync_interval_seconds,
            max_bytes: folder.max_bytes,
        },
        status,
    })
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            syncer_load_snapshot,
            syncer_add_folder,
            syncer_sync_once,
            syncer_retry_failed,
            syncer_resolve_blocked,
            syncer_delete_file
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Syncer Linux app");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_folder_config_with_defaults() {
        let path =
            std::env::temp_dir().join(format!("syncer-linux-test-{}", new_folder_id("docs")));
        fs::create_dir_all(&path).unwrap();

        let config = create_folder_config(
            AddFolderInput {
                display_name: "Docs".to_owned(),
                path: path.clone(),
                mode: "bidirectional".to_owned(),
                max_bytes: None,
                interval_seconds: None,
            },
            &LinuxState {
                selected_folder_id: None,
                folders: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(config.name, "Docs");
        assert_eq!(config.path, path);
        assert_eq!(config.max_bytes, DEFAULT_FOLDER_SIZE_LIMIT_BYTES);
        assert_eq!(config.sync_interval_seconds, DEFAULT_SYNC_INTERVAL_SECONDS);

        fs::remove_dir_all(config.path).unwrap();
    }

    #[test]
    fn rejects_relative_folder_path() {
        let result = create_folder_config(
            AddFolderInput {
                display_name: "Docs".to_owned(),
                path: PathBuf::from("docs"),
                mode: "bidirectional".to_owned(),
                max_bytes: Some(1024),
                interval_seconds: Some(300),
            },
            &LinuxState {
                selected_folder_id: None,
                folders: Vec::new(),
            },
        );

        assert!(matches!(result, Err(AppError::InvalidFolderPath)));
    }
}
