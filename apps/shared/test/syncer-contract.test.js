import test from "node:test";
import assert from "node:assert/strict";

import {
  buildFolderCard,
  formatBytes,
  normalizeOperations,
  summarizeSyncOnce
} from "../src/syncer-contract.js";

test("builds folder cards from agent shaped data", () => {
  const card = buildFolderCard(
    {
      id: "folder-1",
      display_name: "Documents",
      path: "/home/lester/Documents",
      mode: "bidirectional",
      interval_seconds: 300,
      max_bytes: 10_737_418_240
    },
    {
      indexed_files: 4,
      indexed_bytes: 2048,
      skipped_folder_limit: 1,
      unsynchronized_local_missing: 2
    }
  );

  assert.equal(card.name, "Documents");
  assert.equal(card.mode, "bidirectional");
  assert.equal(card.indexedFiles, 4);
});

test("summarizes sync-once output for dashboards", () => {
  const summary = summarizeSyncOnce({
    presence: { accepted: true },
    plan_summary: {
      upload_to_remote: 1,
      download_from_remote: 2,
      conflicts: 1,
      mode_blocked: 0
    },
    transfer: { completed: 3, failed: 0 },
    final_queue: { total: 4, pending: 0, blocked: 1, failed: 0, done: 3 }
  });

  assert.equal(summary.peerOnline, true);
  assert.equal(summary.planned.downloads, 2);
  assert.equal(summary.queue.needsDecision, 1);
});

test("normalizes operations for cross-platform UI lists", () => {
  const operations = normalizeOperations([
    {
      id: "op-1",
      path: "docs/a.txt",
      kind: "resolve_conflict",
      status: "blocked",
      reason: "different content",
      last_error: null,
      retry_count: 0
    }
  ]);

  assert.equal(operations[0].label, "Conflict");
  assert.equal(operations[0].tone, "warning");
});

test("formats byte counts with stable compact units", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(1536), "1.5 KB");
  assert.equal(formatBytes(10_737_418_240), "10 GB");
});
