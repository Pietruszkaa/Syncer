import { formatBytes } from "../../shared/src/syncer-contract.js";
import { createAgentAdapter } from "./agent-adapter.js";
import { buildDashboardViewModel } from "./view-model.js";

const adapter = createAgentAdapter();
let currentModel = null;

document.getElementById("sync-now").addEventListener("click", () => {
  void runSync();
});

void refresh();

async function refresh() {
  setBusy(true);
  try {
    const snapshot = await adapter.loadSnapshot();
    currentModel = buildDashboardViewModel(snapshot);
    render(currentModel);
  } catch (error) {
    renderError(error);
  } finally {
    setBusy(false);
  }
}

async function runSync() {
  if (!currentModel?.selectedFolder) {
    return;
  }
  setBusy(true);
  document.getElementById("peer-state").textContent = "Sync requested";
  try {
    const snapshot = await adapter.syncOnce(currentModel.selectedFolder.id);
    currentModel = buildDashboardViewModel(snapshot);
    render(currentModel);
  } catch (error) {
    renderError(error);
  } finally {
    setBusy(false);
  }
}

export function render(model) {
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

function renderError(error) {
  document.getElementById("peer-state").textContent = "Action failed";
  document.getElementById("operation-list").replaceChildren(errorRow(error));
}

function errorRow(error) {
  const row = document.createElement("article");
  row.className = "operation-row";
  row.innerHTML = `
    <span class="badge danger">Error</span>
    <div>
      <div class="operation-path">Agent command failed</div>
      <div class="operation-reason"></div>
    </div>
    <button class="segment" type="button">Retry</button>
  `;
  row.querySelector(".operation-reason").textContent =
    error instanceof Error ? error.message : String(error);
  row.querySelector("button").addEventListener("click", () => {
    void refresh();
  });
  return row;
}

function setBusy(busy) {
  const syncButton = document.getElementById("sync-now");
  syncButton.disabled = busy;
  syncButton.classList.toggle("busy", busy);
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
