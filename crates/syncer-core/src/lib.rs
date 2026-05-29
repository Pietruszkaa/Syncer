pub mod error;
pub mod folder_store;
pub mod ids;
pub mod limits;
pub mod manifest;
pub mod model;
pub mod planning;
pub mod scanner;
pub mod security;
pub mod state_db;

pub use error::{CoreError, CoreResult};
pub use folder_store::{FolderDocument, FolderStore};
pub use ids::{DeviceId, FolderId, OperationId};
pub use limits::{
    DEFAULT_FOLDER_SIZE_LIMIT_BYTES, FolderLimitDecision, FolderLimitPlan, FolderSizeLimit,
    PendingTransfer,
};
pub use manifest::{BlockHash, ContentHash, FileEntry, FileKind, FileVersion, RelativePath};
pub use model::{
    DeleteScope, DeviceName, FolderConfig, FolderMode, FolderPeer, PeerEndpoint, RetentionPolicy,
    StorageRoot,
};
pub use planning::{
    ChangeClassification, FilePresence, SyncAction, SyncPlan, SyncPlanEntry, SyncPlanSummary,
    classify_file_presence, plan_manifest_sync, plan_manifest_sync_for_mode,
};
pub use scanner::{LinuxFolderScanner, ScanSummary};
pub use security::{PairingTicket, PeerPresence, PublicKeyBytes};
pub use state_db::{
    FileRecord, FileSyncState, FolderManifest, FolderStatus, QueueStatus, RetryFailedSummary,
    SkippedFileRecord, SkippedReason, StateDatabase, SyncOperationKind, SyncOperationRecord,
    SyncOperationStatus,
};
