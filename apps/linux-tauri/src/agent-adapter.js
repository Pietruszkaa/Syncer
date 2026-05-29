import { sampleSnapshot } from "./sample-snapshot.js";

export function createAgentAdapter(invoker = defaultInvoker()) {
  return {
    async loadSnapshot() {
      if (!invoker) {
        return sampleSnapshot;
      }
      return invoker("syncer_load_snapshot");
    },

    async addFolder(folder) {
      if (!invoker) {
        return {
          ...sampleSnapshot,
          selectedFolderId: "new-folder",
          folders: [
            ...sampleSnapshot.folders,
            {
              config: {
                id: "new-folder",
                display_name: folder.displayName,
                path: folder.path,
                mode: folder.mode,
                interval_seconds: folder.intervalSeconds,
                max_bytes: folder.maxBytes
              },
              status: {
                indexed_files: 0,
                indexed_bytes: 0,
                skipped_folder_limit: 0,
                unsynchronized_local_missing: 0
              }
            }
          ]
        };
      }
      return invoker("syncer_add_folder", { folder });
    },

    async syncOnce(folderId) {
      if (!invoker) {
        return {
          ...sampleSnapshot,
          peer: { online: true },
          queue: { total: 3, pending: 0, blocked: 1, done: 2, failed: 0 }
        };
      }
      return invoker("syncer_sync_once", { folderId });
    },

    async retryFailed(folderId) {
      if (!invoker) {
        return sampleSnapshot;
      }
      return invoker("syncer_retry_failed", { folderId });
    },

    async resolveBlocked(operationId, action) {
      if (!invoker) {
        return {
          ...sampleSnapshot,
          operations: sampleSnapshot.operations.filter((operation) => operation.id !== operationId)
        };
      }
      return invoker("syncer_resolve_blocked", { operationId, action });
    },

    async deleteFile(folderId, path, scope) {
      if (!invoker) {
        return {
          ...sampleSnapshot,
          operations: sampleSnapshot.operations.filter((operation) => operation.path !== path)
        };
      }
      return invoker("syncer_delete_file", { folderId, path, scope });
    }
  };
}

function defaultInvoker() {
  return globalThis.__TAURI__?.core?.invoke ?? null;
}
