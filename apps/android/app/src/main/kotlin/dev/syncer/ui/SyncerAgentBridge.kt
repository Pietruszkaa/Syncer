package dev.syncer.ui

interface SyncerAgentBridge {
    suspend fun loadState(): SyncerScreenState

    suspend fun syncOnce(folderId: String): SyncOnceResult

    suspend fun retryFailed(folderId: String)

    suspend fun resolveBlocked(operationId: String, action: ResolveAction)

    suspend fun deleteFile(folderId: String, path: String, scope: DeleteScope)
}

enum class ResolveAction {
    DownloadFromRemote,
    UploadToRemote
}

enum class DeleteScope {
    LocalOnly,
    Both
}

data class SyncOnceResult(
    val peerOnline: Boolean,
    val completed: Long,
    val failed: Long,
    val needsDecision: Long
)
