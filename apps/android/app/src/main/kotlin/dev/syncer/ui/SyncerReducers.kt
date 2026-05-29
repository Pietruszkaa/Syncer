package dev.syncer.ui

fun SyncerScreenState.selectFolder(folderId: String): SyncerScreenState =
    copy(selectedFolderId = folderId)

fun SyncerScreenState.withSyncStarted(): SyncerScreenState =
    copy(activeTransfer = true)

fun SyncerScreenState.withSyncFinished(
    result: SyncOnceResult,
    updatedQueue: QueueSummary,
    updatedOperations: List<OperationRow>
): SyncerScreenState =
    copy(
        peer = peer?.copy(online = result.peerOnline),
        queue = updatedQueue,
        operations = updatedOperations,
        activeTransfer = false
    )

fun SyncerScreenState.withOperationResolved(operationId: String): SyncerScreenState =
    copy(operations = operations.filterNot { it.id == operationId })

fun SyncerScreenState.withPermissionLost(folderId: String): SyncerScreenState =
    copy(
        operations = operations + OperationRow(
            id = "permission-$folderId",
            path = selectedFolder?.path.orEmpty(),
            kind = OperationKind.ModeBlocked,
            status = OperationStatus.Blocked,
            reason = "Folder permission is no longer available",
            lastError = null,
            retryCount = 0
        )
    )
