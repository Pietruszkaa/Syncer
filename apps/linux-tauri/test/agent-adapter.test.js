import test from "node:test";
import assert from "node:assert/strict";

import { createAgentAdapter } from "../src/agent-adapter.js";

test("uses Tauri command names for agent actions", async () => {
  const calls = [];
  const adapter = createAgentAdapter(async (command, payload) => {
    calls.push({ command, payload });
    return { ok: true };
  });

  await adapter.loadSnapshot();
  await adapter.syncOnce("folder-1");
  await adapter.retryFailed("folder-1");
  await adapter.resolveBlocked("op-1", "download_from_remote");
  await adapter.deleteFile("folder-1", "docs/a.txt", "local_only");

  assert.deepEqual(calls, [
    { command: "syncer_load_snapshot", payload: undefined },
    { command: "syncer_sync_once", payload: { folderId: "folder-1" } },
    { command: "syncer_retry_failed", payload: { folderId: "folder-1" } },
    {
      command: "syncer_resolve_blocked",
      payload: { operationId: "op-1", action: "download_from_remote" }
    },
    {
      command: "syncer_delete_file",
      payload: { folderId: "folder-1", path: "docs/a.txt", scope: "local_only" }
    }
  ]);
});

test("falls back to local sample data outside Tauri", async () => {
  const adapter = createAgentAdapter(null);
  const snapshot = await adapter.loadSnapshot();
  const synced = await adapter.syncOnce("documents");

  assert.equal(snapshot.selectedFolderId, "documents");
  assert.equal(synced.queue.pending, 0);
});
