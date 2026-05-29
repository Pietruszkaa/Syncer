mod agent_command;

use agent_command::{
    delete_file as build_delete_file, queue_status as build_queue_status,
    resolve_blocked as build_resolve_blocked, retry_failed as build_retry_failed,
    sync_once as build_sync_once, AgentCommandError, AgentRuntime, FolderConnection,
    PeerConnection,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use thiserror::Error;

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
    folders: Vec<FolderConfig>,
    peer: PeerSnapshot,
    queue: QueueSummary,
    operations: Vec<serde_json::Value>,
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
fn syncer_sync_once(app: AppHandle, folder_id: String) -> Result<serde_json::Value, AppError> {
    let context = load_context(&app)?;
    let folder = context.folder(&folder_id)?;
    let command = build_sync_once(&context.runtime, &folder.connection(), 100)?;
    command.run_json().map_err(AppError::from)
}

#[tauri::command]
fn syncer_retry_failed(app: AppHandle, folder_id: String) -> Result<serde_json::Value, AppError> {
    let context = load_context(&app)?;
    let folder = context.folder(&folder_id)?;
    let command = build_retry_failed(&context.runtime, &folder.connection());
    command.run_json().map_err(AppError::from)
}

#[tauri::command]
fn syncer_resolve_blocked(
    app: AppHandle,
    operation_id: String,
    action: String,
) -> Result<serde_json::Value, AppError> {
    let context = load_context(&app)?;
    let folder = context.selected_folder()?;
    let command = build_resolve_blocked(
        &context.runtime,
        &folder.connection(),
        &operation_id,
        &action,
    );
    command.run_json().map_err(AppError::from)
}

#[tauri::command]
fn syncer_delete_file(
    app: AppHandle,
    folder_id: String,
    path: String,
    scope: String,
) -> Result<serde_json::Value, AppError> {
    let context = load_context(&app)?;
    let folder = context.folder(&folder_id)?;
    let command = build_delete_file(&context.runtime, &folder.connection(), &path, &scope)?;
    command.run_json().map_err(AppError::from)
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

fn load_snapshot(context: &AppContext) -> Result<DashboardSnapshot, AppError> {
    let selected_folder = context.selected_folder().ok();
    let queue_output = selected_folder
        .map(|folder| build_queue_status(&context.runtime, &folder.connection()).run_json())
        .transpose()?;

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

    Ok(DashboardSnapshot {
        selected_folder_id: context.state.selected_folder_id.clone(),
        folders: context.state.folders.clone(),
        peer,
        queue,
        operations,
    })
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            syncer_load_snapshot,
            syncer_sync_once,
            syncer_retry_failed,
            syncer_resolve_blocked,
            syncer_delete_file
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Syncer Linux app");
}
