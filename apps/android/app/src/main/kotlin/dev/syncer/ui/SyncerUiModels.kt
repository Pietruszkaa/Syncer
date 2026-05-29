package dev.syncer.ui

enum class FolderMode {
    Bidirectional,
    UploadOnly,
    DownloadOnly
}

enum class OperationKind {
    UploadToRemote,
    DownloadFromRemote,
    ResolveConflict,
    RepairLocalMissing,
    RepairRemoteMissing,
    ModeBlocked
}

enum class OperationStatus {
    Pending,
    Blocked,
    Done,
    Failed
}

data class FolderCard(
    val id: String,
    val name: String,
    val path: String,
    val mode: FolderMode,
    val intervalSeconds: Long,
    val maxBytes: Long,
    val indexedBytes: Long,
    val indexedFiles: Long,
    val skippedFolderLimit: Long,
    val unsynchronizedLocalMissing: Long
)

data class QueueSummary(
    val total: Long,
    val actionable: Long,
    val needsDecision: Long,
    val failed: Long,
    val done: Long
)

data class OperationRow(
    val id: String,
    val path: String,
    val kind: OperationKind,
    val status: OperationStatus,
    val reason: String,
    val lastError: String?,
    val retryCount: Long
)

data class PeerState(
    val deviceId: String,
    val name: String,
    val endpoint: String?,
    val online: Boolean
)

data class SyncerScreenState(
    val onboardingRequired: Boolean,
    val selectedFolderId: String?,
    val folders: List<FolderCard>,
    val peer: PeerState?,
    val queue: QueueSummary,
    val operations: List<OperationRow>,
    val activeTransfer: Boolean
) {
    val selectedFolder: FolderCard?
        get() = folders.firstOrNull { it.id == selectedFolderId } ?: folders.firstOrNull()
}
