use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::FileEntry;
use crate::model::FolderMode;
use crate::state_db::{FileRecord, FileSyncState, FolderManifest};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FilePresence {
    Present(FileEntry),
    MissingKnown,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ChangeClassification {
    InSync,
    LocalOnlyDelete,
    RemoteOnlyDelete,
    LocalNeedsDownload,
    RemoteNeedsUpload,
    Conflict,
    Unknown,
}

#[must_use]
pub fn classify_file_presence(
    local: &FilePresence,
    remote: &FilePresence,
    common_base: Option<&FileEntry>,
) -> ChangeClassification {
    match (local, remote) {
        (FilePresence::Present(left), FilePresence::Present(right)) => {
            if left.content_hash == right.content_hash && left.size_bytes == right.size_bytes {
                ChangeClassification::InSync
            } else if let Some(base) = common_base {
                let local_changed = left.content_hash != base.content_hash;
                let remote_changed = right.content_hash != base.content_hash;

                match (local_changed, remote_changed) {
                    (true, true) => ChangeClassification::Conflict,
                    (true, false) => ChangeClassification::RemoteNeedsUpload,
                    (false, true) => ChangeClassification::LocalNeedsDownload,
                    (false, false) => ChangeClassification::InSync,
                }
            } else {
                ChangeClassification::Conflict
            }
        }
        (FilePresence::MissingKnown, FilePresence::Present(_)) => {
            ChangeClassification::LocalOnlyDelete
        }
        (FilePresence::Present(_), FilePresence::MissingKnown) => {
            ChangeClassification::RemoteOnlyDelete
        }
        (FilePresence::Unknown, _) | (_, FilePresence::Unknown) => ChangeClassification::Unknown,
        (FilePresence::MissingKnown, FilePresence::MissingKnown) => ChangeClassification::InSync,
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SyncPlan {
    pub local_device_id: String,
    pub remote_device_id: String,
    pub entries: Vec<SyncPlanEntry>,
    pub summary: SyncPlanSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SyncPlanEntry {
    pub path: String,
    pub action: SyncAction,
    pub local_size_bytes: Option<u64>,
    pub remote_size_bytes: Option<u64>,
    pub local_content_hash: Option<String>,
    pub remote_content_hash: Option<String>,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    InSync,
    UploadToRemote,
    DownloadFromRemote,
    Conflict,
    LocalMissingUnsynchronized,
    RemoteMissingUnsynchronized,
    ModeBlocked,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SyncPlanSummary {
    pub in_sync: u64,
    pub upload_to_remote: u64,
    pub download_from_remote: u64,
    pub conflicts: u64,
    pub local_missing_unsynchronized: u64,
    pub remote_missing_unsynchronized: u64,
    pub mode_blocked: u64,
}

#[must_use]
pub fn plan_manifest_sync(local: &FolderManifest, remote: &FolderManifest) -> SyncPlan {
    plan_manifest_sync_for_mode(local, remote, FolderMode::Bidirectional)
}

#[must_use]
pub fn plan_manifest_sync_for_mode(
    local: &FolderManifest,
    remote: &FolderManifest,
    mode: FolderMode,
) -> SyncPlan {
    let local_by_path = files_by_path(&local.files);
    let remote_by_path = files_by_path(&remote.files);
    let paths = local_by_path
        .keys()
        .chain(remote_by_path.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut plan = SyncPlan {
        local_device_id: local.device_id.to_string(),
        remote_device_id: remote.device_id.to_string(),
        entries: Vec::new(),
        summary: SyncPlanSummary::default(),
    };

    for path in paths {
        let local_file = local_by_path.get(&path);
        let remote_file = remote_by_path.get(&path);
        let entry = apply_folder_mode(plan_path(&path, local_file, remote_file), mode);
        increment_summary(&mut plan.summary, entry.action);
        plan.entries.push(entry);
    }

    plan
}

fn apply_folder_mode(mut entry: SyncPlanEntry, mode: FolderMode) -> SyncPlanEntry {
    let blocked = match (mode, entry.action) {
        (FolderMode::UploadOnly, SyncAction::DownloadFromRemote) => Some("folder is upload-only"),
        (FolderMode::DownloadOnly, SyncAction::UploadToRemote) => Some("folder is download-only"),
        _ => None,
    };

    if let Some(reason) = blocked {
        entry.action = SyncAction::ModeBlocked;
        reason.clone_into(&mut entry.reason);
    }

    entry
}

fn files_by_path(files: &[FileRecord]) -> BTreeMap<String, &FileRecord> {
    files
        .iter()
        .map(|file| (file.path.as_path().as_str().to_owned(), file))
        .collect()
}

fn plan_path(
    path: &str,
    local_file: Option<&&FileRecord>,
    remote_file: Option<&&FileRecord>,
) -> SyncPlanEntry {
    match (local_file, remote_file) {
        (Some(local), Some(remote)) => plan_existing_path(path, local, remote),
        (Some(local), None) => {
            if local.sync_state == FileSyncState::UnsynchronizedLocalMissing {
                entry(
                    path,
                    SyncAction::LocalMissingUnsynchronized,
                    Some(local.size_bytes),
                    None,
                    local.content_hash.clone(),
                    None,
                    "local side is missing a previously known file",
                )
            } else {
                entry(
                    path,
                    SyncAction::UploadToRemote,
                    Some(local.size_bytes),
                    None,
                    local.content_hash.clone(),
                    None,
                    "remote side does not have this file",
                )
            }
        }
        (None, Some(remote)) => {
            if remote.sync_state == FileSyncState::UnsynchronizedLocalMissing {
                entry(
                    path,
                    SyncAction::RemoteMissingUnsynchronized,
                    None,
                    Some(remote.size_bytes),
                    None,
                    remote.content_hash.clone(),
                    "remote side is missing a previously known file",
                )
            } else {
                entry(
                    path,
                    SyncAction::DownloadFromRemote,
                    None,
                    Some(remote.size_bytes),
                    None,
                    remote.content_hash.clone(),
                    "local side does not have this file",
                )
            }
        }
        (None, None) => entry(
            path,
            SyncAction::InSync,
            None,
            None,
            None,
            None,
            "file absent on both sides",
        ),
    }
}

fn plan_existing_path(path: &str, local: &FileRecord, remote: &FileRecord) -> SyncPlanEntry {
    if local.sync_state == FileSyncState::UnsynchronizedLocalMissing {
        return entry(
            path,
            SyncAction::LocalMissingUnsynchronized,
            Some(local.size_bytes),
            Some(remote.size_bytes),
            local.content_hash.clone(),
            remote.content_hash.clone(),
            "local side is missing a previously known file",
        );
    }
    if remote.sync_state == FileSyncState::UnsynchronizedLocalMissing {
        return entry(
            path,
            SyncAction::RemoteMissingUnsynchronized,
            Some(local.size_bytes),
            Some(remote.size_bytes),
            local.content_hash.clone(),
            remote.content_hash.clone(),
            "remote side is missing a previously known file",
        );
    }
    if local.content_hash == remote.content_hash && local.size_bytes == remote.size_bytes {
        return entry(
            path,
            SyncAction::InSync,
            Some(local.size_bytes),
            Some(remote.size_bytes),
            local.content_hash.clone(),
            remote.content_hash.clone(),
            "content hash and size match",
        );
    }

    entry(
        path,
        SyncAction::Conflict,
        Some(local.size_bytes),
        Some(remote.size_bytes),
        local.content_hash.clone(),
        remote.content_hash.clone(),
        "both sides have different content and no common base version is known",
    )
}

fn entry(
    path: &str,
    action: SyncAction,
    local_size_bytes: Option<u64>,
    remote_size_bytes: Option<u64>,
    local_content_hash: Option<String>,
    remote_content_hash: Option<String>,
    reason: &str,
) -> SyncPlanEntry {
    SyncPlanEntry {
        path: path.to_owned(),
        action,
        local_size_bytes,
        remote_size_bytes,
        local_content_hash,
        remote_content_hash,
        reason: reason.to_owned(),
    }
}

fn increment_summary(summary: &mut SyncPlanSummary, action: SyncAction) {
    match action {
        SyncAction::InSync => summary.in_sync = summary.in_sync.saturating_add(1),
        SyncAction::UploadToRemote => {
            summary.upload_to_remote = summary.upload_to_remote.saturating_add(1);
        }
        SyncAction::DownloadFromRemote => {
            summary.download_from_remote = summary.download_from_remote.saturating_add(1);
        }
        SyncAction::Conflict => summary.conflicts = summary.conflicts.saturating_add(1),
        SyncAction::LocalMissingUnsynchronized => {
            summary.local_missing_unsynchronized =
                summary.local_missing_unsynchronized.saturating_add(1);
        }
        SyncAction::RemoteMissingUnsynchronized => {
            summary.remote_missing_unsynchronized =
                summary.remote_missing_unsynchronized.saturating_add(1);
        }
        SyncAction::ModeBlocked => {
            summary.mode_blocked = summary.mode_blocked.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use crate::{
        DeviceId, FileKind, FileRecord, FileSyncState, FolderId, FolderManifest, RelativePath,
        plan_manifest_sync, plan_manifest_sync_for_mode,
    };

    #[test]
    fn plans_upload_download_conflict_and_in_sync() -> Result<(), crate::CoreError> {
        let local_device_id = DeviceId::new();
        let remote_device_id = DeviceId::new();
        let local = manifest(
            local_device_id,
            vec![
                record("same.txt", 10, "same", FileSyncState::LocalAvailable)?,
                record("local-only.txt", 10, "local", FileSyncState::LocalAvailable)?,
                record("conflict.txt", 10, "local", FileSyncState::LocalAvailable)?,
            ],
        );
        let remote = manifest(
            remote_device_id,
            vec![
                record("same.txt", 10, "same", FileSyncState::LocalAvailable)?,
                record(
                    "remote-only.txt",
                    10,
                    "remote",
                    FileSyncState::LocalAvailable,
                )?,
                record("conflict.txt", 10, "remote", FileSyncState::LocalAvailable)?,
            ],
        );

        let plan = plan_manifest_sync(&local, &remote);

        assert_eq!(plan.summary.in_sync, 1);
        assert_eq!(plan.summary.upload_to_remote, 1);
        assert_eq!(plan.summary.download_from_remote, 1);
        assert_eq!(plan.summary.conflicts, 1);
        Ok(())
    }

    #[test]
    fn plans_unsynchronized_missing_without_delete() -> Result<(), crate::CoreError> {
        let local = manifest(
            DeviceId::new(),
            vec![record(
                "gone.txt",
                10,
                "old",
                FileSyncState::UnsynchronizedLocalMissing,
            )?],
        );
        let remote = manifest(
            DeviceId::new(),
            vec![record(
                "gone.txt",
                10,
                "old",
                FileSyncState::LocalAvailable,
            )?],
        );

        let plan = plan_manifest_sync(&local, &remote);

        assert_eq!(plan.summary.local_missing_unsynchronized, 1);
        assert_eq!(
            plan.entries[0].action,
            crate::SyncAction::LocalMissingUnsynchronized
        );
        Ok(())
    }

    #[test]
    fn blocks_disallowed_folder_mode_actions() -> Result<(), crate::CoreError> {
        let local = manifest(
            DeviceId::new(),
            vec![record(
                "local-only.txt",
                10,
                "local",
                FileSyncState::LocalAvailable,
            )?],
        );
        let remote = manifest(
            DeviceId::new(),
            vec![record(
                "remote-only.txt",
                10,
                "remote",
                FileSyncState::LocalAvailable,
            )?],
        );

        let upload_only =
            plan_manifest_sync_for_mode(&local, &remote, crate::FolderMode::UploadOnly);
        let download_only =
            plan_manifest_sync_for_mode(&local, &remote, crate::FolderMode::DownloadOnly);

        assert_eq!(upload_only.summary.upload_to_remote, 1);
        assert_eq!(upload_only.summary.mode_blocked, 1);
        assert!(upload_only.entries.iter().any(|entry| {
            entry.path == "remote-only.txt" && entry.action == crate::SyncAction::ModeBlocked
        }));
        assert_eq!(download_only.summary.download_from_remote, 1);
        assert_eq!(download_only.summary.mode_blocked, 1);
        assert!(download_only.entries.iter().any(|entry| {
            entry.path == "local-only.txt" && entry.action == crate::SyncAction::ModeBlocked
        }));
        Ok(())
    }

    fn manifest(device_id: DeviceId, files: Vec<FileRecord>) -> FolderManifest {
        FolderManifest {
            folder_id: FolderId::new(),
            device_id,
            generated_at: OffsetDateTime::now_utc(),
            files,
        }
    }

    fn record(
        path: &str,
        size_bytes: u64,
        hash: &str,
        sync_state: FileSyncState,
    ) -> Result<FileRecord, crate::CoreError> {
        Ok(FileRecord {
            path: RelativePath::parse(path)?,
            kind: FileKind::File,
            size_bytes,
            modified_at: OffsetDateTime::now_utc(),
            content_hash: Some(hash.to_owned()),
            generation: 1,
            device_id: DeviceId::new().to_string(),
            sync_state,
        })
    }
}
