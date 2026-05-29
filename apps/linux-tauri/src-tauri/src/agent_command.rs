use serde::de::DeserializeOwned;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct AgentRuntime {
    pub binary: PathBuf,
    pub device_config: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PeerConnection {
    pub endpoint: String,
    pub shared_secret: String,
}

#[derive(Clone, Debug)]
pub struct FolderConnection {
    pub path: PathBuf,
    pub peer: Option<PeerConnection>,
}

#[derive(Clone, Debug)]
pub struct AgentCommand {
    pub binary: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Error)]
pub enum AgentCommandError {
    #[error("syncer-agent is not configured")]
    MissingAgentBinary,
    #[error("folder has no online peer configuration")]
    MissingPeer,
    #[error("syncer-agent failed with status {status}: {stderr}")]
    Failed { status: i32, stderr: String },
    #[error("syncer-agent output is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("failed to run syncer-agent: {0}")]
    Io(#[from] std::io::Error),
}

impl AgentRuntime {
    pub fn from_env_or_default(app_data_dir: &Path) -> Self {
        let binary = std::env::var_os("SYNCER_AGENT_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("syncer-agent"));
        let device_config = std::env::var_os("SYNCER_DEVICE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| app_data_dir.join("device.json"));

        Self {
            binary,
            device_config,
        }
    }
}

impl AgentCommand {
    pub fn new(binary: PathBuf, args: Vec<OsString>) -> Self {
        Self { binary, args }
    }

    pub fn run_json<T: DeserializeOwned>(&self) -> Result<T, AgentCommandError> {
        if self.binary.as_os_str().is_empty() {
            return Err(AgentCommandError::MissingAgentBinary);
        }

        let output = Command::new(&self.binary).args(&self.args).output()?;
        if !output.status.success() {
            return Err(AgentCommandError::Failed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

pub fn queue_status(runtime: &AgentRuntime, folder: &FolderConnection) -> AgentCommand {
    AgentCommand::new(
        runtime.binary.clone(),
        vec![
            os("queue-status"),
            os("--path"),
            folder.path.clone().into_os_string(),
        ],
    )
}

pub fn retry_failed(runtime: &AgentRuntime, folder: &FolderConnection) -> AgentCommand {
    AgentCommand::new(
        runtime.binary.clone(),
        vec![
            os("retry-failed"),
            os("--path"),
            folder.path.clone().into_os_string(),
        ],
    )
}

pub fn resolve_blocked(
    runtime: &AgentRuntime,
    folder: &FolderConnection,
    operation_id: &str,
    action: &str,
) -> AgentCommand {
    AgentCommand::new(
        runtime.binary.clone(),
        vec![
            os("resolve-blocked"),
            os("--path"),
            folder.path.clone().into_os_string(),
            os("--operation-id"),
            os(operation_id),
            os("--action"),
            os(action),
        ],
    )
}

pub fn sync_once(
    runtime: &AgentRuntime,
    folder: &FolderConnection,
    limit: u64,
) -> Result<AgentCommand, AgentCommandError> {
    let peer = folder.peer.as_ref().ok_or(AgentCommandError::MissingPeer)?;
    Ok(AgentCommand::new(
        runtime.binary.clone(),
        vec![
            os("sync-once"),
            os("--path"),
            folder.path.clone().into_os_string(),
            os("--endpoint"),
            os(&peer.endpoint),
            os("--shared-secret"),
            os(&peer.shared_secret),
            os("--limit"),
            os(limit.to_string()),
            os("--device-config"),
            runtime.device_config.clone().into_os_string(),
        ],
    ))
}

pub fn delete_file(
    runtime: &AgentRuntime,
    folder: &FolderConnection,
    file: &str,
    scope: &str,
) -> Result<AgentCommand, AgentCommandError> {
    let peer = folder.peer.as_ref();
    let mut args = vec![
        os("delete-file"),
        os("--path"),
        folder.path.clone().into_os_string(),
        os("--file"),
        os(file),
        os("--scope"),
        os(scope),
        os("--device-config"),
        runtime.device_config.clone().into_os_string(),
    ];

    if scope == "both_sides" {
        let peer = peer.ok_or(AgentCommandError::MissingPeer)?;
        args.extend([
            os("--endpoint"),
            os(&peer.endpoint),
            os("--shared-secret"),
            os(&peer.shared_secret),
        ]);
    }

    Ok(AgentCommand::new(runtime.binary.clone(), args))
}

fn os(value: impl AsRef<OsStr>) -> OsString {
    value.as_ref().to_os_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> AgentRuntime {
        AgentRuntime {
            binary: PathBuf::from("syncer-agent"),
            device_config: PathBuf::from("/tmp/device.json"),
        }
    }

    fn folder() -> FolderConnection {
        FolderConnection {
            path: PathBuf::from("/tmp/folder"),
            peer: Some(PeerConnection {
                endpoint: "http://127.0.0.1:57421".to_owned(),
                shared_secret: "secret".to_owned(),
            }),
        }
    }

    #[test]
    fn builds_sync_once_command() {
        let command = sync_once(&runtime(), &folder(), 25).unwrap();

        assert_eq!(command.binary, PathBuf::from("syncer-agent"));
        assert_eq!(
            command.args,
            vec![
                os("sync-once"),
                os("--path"),
                os("/tmp/folder"),
                os("--endpoint"),
                os("http://127.0.0.1:57421"),
                os("--shared-secret"),
                os("secret"),
                os("--limit"),
                os("25"),
                os("--device-config"),
                os("/tmp/device.json"),
            ]
        );
    }

    #[test]
    fn local_delete_does_not_require_peer() {
        let mut folder = folder();
        folder.peer = None;

        let command = delete_file(&runtime(), &folder, "docs/a.txt", "local_only").unwrap();

        assert!(!command.args.iter().any(|arg| arg == "--endpoint"));
    }
}
