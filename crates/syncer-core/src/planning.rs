use serde::{Deserialize, Serialize};

use crate::manifest::FileEntry;

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
