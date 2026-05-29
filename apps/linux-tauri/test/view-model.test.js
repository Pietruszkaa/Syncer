import test from "node:test";
import assert from "node:assert/strict";

import { buildDashboardViewModel } from "../src/view-model.js";

test("builds dashboard view model from shared contract data", () => {
  const model = buildDashboardViewModel({
    selectedFolderId: "docs",
    peer: { online: true },
    folders: [
      {
        config: {
          id: "docs",
          display_name: "Docs",
          path: "/tmp/docs",
          mode: "bidirectional",
          interval_seconds: 300,
          max_bytes: 1024
        },
        status: {
          indexed_files: 2,
          indexed_bytes: 512,
          skipped_folder_limit: 0,
          unsynchronized_local_missing: 0
        }
      }
    ],
    queue: { total: 1, pending: 0, blocked: 1, done: 0, failed: 0 },
    operations: [
      {
        id: "op-1",
        path: "a.txt",
        kind: "resolve_conflict",
        status: "blocked",
        reason: "different content",
        last_error: null,
        retry_count: 0
      }
    ]
  });

  assert.equal(model.selectedFolder.name, "Docs");
  assert.equal(model.peerOnline, true);
  assert.equal(model.queue.needsDecision, 1);
  assert.equal(model.operations[0].label, "Conflict");
});
