import {
  buildFolderCard,
  formatBytes,
  formatFolderMode,
  normalizeOperations,
  summarizeQueue
} from "../../shared/src/syncer-contract.js";

export function buildDashboardViewModel(snapshot) {
  const folders = snapshot.folders.map((folder) =>
    buildFolderCard(folder.config, folder.status)
  );
  const selectedFolderId = snapshot.selectedFolderId ?? folders[0]?.id ?? null;
  const selectedFolder = folders.find((folder) => folder.id === selectedFolderId) ?? folders[0] ?? null;
  const operations = normalizeOperations(snapshot.operations ?? []);
  const queue = summarizeQueue(snapshot.queue);

  return {
    folders,
    selectedFolder,
    operations,
    queue,
    peerOnline: Boolean(snapshot.peer?.online),
    settings: selectedFolder
      ? [
          { label: "Mode", value: formatFolderMode(selectedFolder.mode) },
          { label: "Interval", value: `${selectedFolder.intervalSeconds}s` },
          { label: "Limit", value: formatBytes(selectedFolder.maxBytes) }
        ]
      : []
  };
}
