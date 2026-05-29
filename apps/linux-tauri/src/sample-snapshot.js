export const sampleSnapshot = {
  selectedFolderId: "documents",
  peer: { online: true },
  folders: [
    {
      config: {
        id: "documents",
        display_name: "Documents",
        path: "/home/lester/Documents",
        mode: "bidirectional",
        interval_seconds: 300,
        max_bytes: 10_737_418_240
      },
      status: {
        indexed_files: 4,
        indexed_bytes: 16384,
        skipped_folder_limit: 0,
        unsynchronized_local_missing: 1
      }
    },
    {
      config: {
        id: "phone",
        display_name: "Phone camera",
        path: "/home/lester/Pictures/Phone",
        mode: "download_only",
        interval_seconds: 900,
        max_bytes: 5_368_709_120
      },
      status: {
        indexed_files: 128,
        indexed_bytes: 904_192_000,
        skipped_folder_limit: 2,
        unsynchronized_local_missing: 0
      }
    }
  ],
  queue: {
    total: 3,
    pending: 1,
    blocked: 1,
    done: 1,
    failed: 0
  },
  operations: [
    {
      id: "op-1",
      path: "notes/today.md",
      kind: "download_from_remote",
      status: "pending",
      reason: "local side does not have this file",
      last_error: null,
      retry_count: 0
    },
    {
      id: "op-2",
      path: "projects/syncer.md",
      kind: "resolve_conflict",
      status: "blocked",
      reason: "both sides have different content",
      last_error: null,
      retry_count: 0
    },
    {
      id: "op-3",
      path: "archive/old.txt",
      kind: "upload_to_remote",
      status: "done",
      reason: "remote side does not have this file",
      last_error: null,
      retry_count: 0
    }
  ]
};
