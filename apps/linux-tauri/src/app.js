import { formatBytes } from "../../shared/src/syncer-contract.js";
import { buildDashboardViewModel } from "./view-model.js";

const snapshot = {
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

const viewModel = buildDashboardViewModel(snapshot);

render(viewModel);

document.getElementById("sync-now").addEventListener("click", () => {
  document.getElementById("peer-state").textContent = "Sync requested";
});

function render(model) {
  renderFolders(model);
  renderSummary(model);
  renderOperations(model.operations);
  renderSettings(model.settings);
}

function renderFolders(model) {
  const list = document.getElementById("folder-list");
  list.replaceChildren(
    ...model.folders.map((folder) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = `folder-button${folder.id === model.selectedFolder?.id ? " active" : ""}`;
      button.innerHTML = `
        <span class="folder-name"></span>
        <span class="folder-meta"></span>
      `;
      button.querySelector(".folder-name").textContent = folder.name;
      button.querySelector(".folder-meta").textContent = folder.path;
      return button;
    })
  );
}

function renderSummary(model) {
  const folder = model.selectedFolder;
  document.getElementById("peer-state").textContent = model.peerOnline ? "Peer online" : "Peer offline";
  document.getElementById("folder-title").textContent = folder?.name ?? "No folder";
  document.getElementById("used-bytes").textContent = folder ? formatBytes(folder.indexedBytes) : "0 B";
  document.getElementById("indexed-files").textContent = String(folder?.indexedFiles ?? 0);
  document.getElementById("pending-count").textContent = String(model.queue.actionable);
  document.getElementById("blocked-count").textContent = String(model.queue.needsDecision);
}

function renderOperations(operations) {
  const list = document.getElementById("operation-list");
  list.replaceChildren(
    ...operations.map((operation) => {
      const row = document.createElement("article");
      row.className = "operation-row";
      row.innerHTML = `
        <span class="badge"></span>
        <div>
          <div class="operation-path"></div>
          <div class="operation-reason"></div>
        </div>
        <button class="segment" type="button">Open</button>
      `;
      const badge = row.querySelector(".badge");
      badge.className = `badge ${operation.tone}`;
      badge.textContent = operation.label;
      row.querySelector(".operation-path").textContent = operation.path;
      row.querySelector(".operation-reason").textContent = operation.error ?? operation.reason;
      return row;
    })
  );
}

function renderSettings(settings) {
  const grid = document.getElementById("folder-settings");
  grid.replaceChildren(
    ...settings.map((setting) => {
      const item = document.createElement("div");
      item.className = "setting";
      item.innerHTML = `
        <span class="setting-label"></span>
        <span class="setting-value"></span>
      `;
      item.querySelector(".setting-label").textContent = setting.label;
      item.querySelector(".setting-value").textContent = setting.value;
      return item;
    })
  );
}
