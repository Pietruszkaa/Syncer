import { sampleSnapshot } from "./sample-snapshot.js";

export function createAgentAdapter(invoker = defaultInvoker()) {
  return {
    async loadSnapshot() {
      if (!invoker) {
        return sampleSnapshot;
      }
      return invoker("syncer_load_snapshot");
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
