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
pub use limits::{FolderLimitDecision, FolderLimitPlan, FolderSizeLimit, PendingTransfer};
pub use manifest::{BlockHash, ContentHash, FileEntry, FileKind, FileVersion, RelativePath};
pub use model::{
    DeleteScope, DeviceName, FolderConfig, FolderMode, FolderPeer, PeerEndpoint, RetentionPolicy,
    StorageRoot,
};
pub use planning::{ChangeClassification, FilePresence, classify_file_presence};
pub use scanner::{LinuxFolderScanner, ScanSummary};
pub use security::{PairingTicket, PeerPresence, PublicKeyBytes};
pub use state_db::{FileRecord, StateDatabase};
