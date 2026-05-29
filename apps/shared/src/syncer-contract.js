export const folderModes = Object.freeze([
  "bidirectional",
  "upload_only",
  "download_only"
]);

export const operationKinds = Object.freeze([
  "upload_to_remote",
  "download_from_remote",
  "resolve_conflict",
  "repair_local_missing",
  "repair_remote_missing",
  "mode_blocked"
]);

export const operationStatuses = Object.freeze([
  "pending",
  "blocked",
  "done",
  "failed"
]);

export function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

export function formatFolderMode(mode) {
  switch (mode) {
    case "upload_only":
      return "Upload only";
    case "download_only":
      return "Download only";
    default:
      return "Two-way";
  }
}

export function operationLabel(kind) {
  switch (kind) {
    case "upload_to_remote":
      return "Upload";
    case "download_from_remote":
      return "Download";
    case "resolve_conflict":
      return "Conflict";
    case "repair_local_missing":
      return "Missing locally";
    case "repair_remote_missing":
      return "Missing remotely";
    case "mode_blocked":
      return "Blocked by folder mode";
    default:
      return "Unknown";
  }
}

export function statusTone(status) {
  switch (status) {
    case "done":
      return "ok";
    case "failed":
      return "danger";
    case "blocked":
      return "warning";
    default:
      return "neutral";
  }
}

export function buildFolderCard(folder, status) {
  return {
    id: requireString(folder.id, "folder.id"),
    name: requireString(folder.display_name, "folder.display_name"),
    path: requireString(folder.path, "folder.path"),
    mode: requireEnum(folder.mode, folderModes, "folder.mode"),
    intervalSeconds: requireInteger(folder.interval_seconds, "folder.interval_seconds"),
    maxBytes: requireInteger(folder.max_bytes, "folder.max_bytes"),
    indexedBytes: requireInteger(status.indexed_bytes, "status.indexed_bytes"),
    indexedFiles: requireInteger(status.indexed_files, "status.indexed_files"),
    skippedFolderLimit: requireInteger(status.skipped_folder_limit, "status.skipped_folder_limit"),
    unsynchronizedLocalMissing: requireInteger(
      status.unsynchronized_local_missing,
      "status.unsynchronized_local_missing"
    )
  };
}

export function summarizeQueue(queue) {
  return {
    total: requireInteger(queue.total, "queue.total"),
    actionable: requireInteger(queue.pending, "queue.pending"),
    needsDecision: requireInteger(queue.blocked, "queue.blocked"),
    failed: requireInteger(queue.failed, "queue.failed"),
    done: requireInteger(queue.done, "queue.done")
  };
}

export function summarizeSyncOnce(output) {
  return {
    peerOnline: Boolean(output.presence?.accepted),
    planned: {
      uploads: requireInteger(output.plan_summary.upload_to_remote, "plan_summary.upload_to_remote"),
      downloads: requireInteger(
        output.plan_summary.download_from_remote,
        "plan_summary.download_from_remote"
      ),
      conflicts: requireInteger(output.plan_summary.conflicts, "plan_summary.conflicts"),
      blockedByMode: requireInteger(output.plan_summary.mode_blocked, "plan_summary.mode_blocked")
    },
    transfer: {
      completed: requireInteger(output.transfer.completed, "transfer.completed"),
      failed: requireInteger(output.transfer.failed, "transfer.failed")
    },
    queue: summarizeQueue(output.final_queue)
  };
}

export function normalizeOperations(operations) {
  return operations.map((operation) => ({
    id: requireString(operation.id, "operation.id"),
    path: requireString(operation.path, "operation.path"),
    kind: requireEnum(operation.kind, operationKinds, "operation.kind"),
    status: requireEnum(operation.status, operationStatuses, "operation.status"),
    label: operationLabel(operation.kind),
    tone: statusTone(operation.status),
    reason: String(operation.reason ?? ""),
    error: operation.last_error ? String(operation.last_error) : null,
    retryCount: requireInteger(operation.retry_count ?? 0, "operation.retry_count")
  }));
}

function requireString(value, name) {
  if (typeof value !== "string" || value.length === 0) {
    throw new TypeError(`${name} must be a non-empty string`);
  }
  return value;
}

function requireInteger(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new TypeError(`${name} must be a non-negative safe integer`);
  }
  return value;
}

function requireEnum(value, allowed, name) {
  if (!allowed.includes(value)) {
    throw new TypeError(`${name} has unsupported value ${String(value)}`);
  }
  return value;
}
