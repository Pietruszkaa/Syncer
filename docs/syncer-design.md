# Syncer Design

## Understanding Summary

- Syncer is a new file synchronization application, not a wrapper around Syncthing.
- MVP targets one user synchronizing their own devices over LAN or VPN, such as Tailscale.
- Initial platforms are Linux desktop and Android.
- The product priority is reliable onboarding: name device, pair peer, choose folder, choose sync direction.
- There are no accounts, no cloud service, and no central pairing server in MVP.
- Synchronization is configured per folder and defaults to bidirectional mode.
- The implementation stack is Rust for the sync core and protocol, Tauri for Linux UI, and Kotlin for Android UI.

## Assumptions

- MVP scale target is up to 5 devices, 25 synchronized folders, and hundreds of thousands of files.
- Synchronization is manual and periodic, with a configurable interval.
- Background operation is required: Linux service/autostart, Android background work, persistent queues, retry, offline state, and notifications.
- File content and sync metadata are encrypted in transport from MVP.
- Device pairing is local through QR code or manual code entry.
- Folder metadata is stored in a hidden `.syncer/` directory inside each synchronized folder.
- Android accesses only user-selected folders through Storage Access Framework.
- Deletions outside Syncer are local-only and do not propagate automatically.
- Deletions from Syncer expose a user choice: local-only delete or propagated delete.
- Conflicts are shown to the user and block automatic overwrite for the affected file.
- Large transfers are resumable from the last verified block.
- Each folder has simple version retention by last N versions or last X days.
- The project is private-first but should be clean enough to publish on GitHub.

## Recommended Architecture

Each device runs a local Syncer Core agent written in Rust. The agent is the source of truth for indexing, manifest comparison, encrypted transfer, versioning, conflict detection, retry queues, peer presence, and synchronization state. Platform UIs are thin control surfaces that send commands to the local agent and subscribe to status updates.

On Linux, Syncer Core runs as a `systemd --user` service with autostart. The Tauri desktop app controls onboarding, device pairing, folder setup, sync status, transfer progress, conflicts, versions, and settings. Closing the UI does not stop synchronization.

On Android, Kotlin UI integrates with Rust core through UniFFI or JNI. Background operation uses WorkManager for periodic work and a Foreground Service during active transfers when required by Android. Because Android folder access uses Storage Access Framework, the Android layer exposes a safe file-access adapter to the Rust core instead of assuming normal POSIX paths.

Peer-to-peer communication runs directly over LAN or VPN. Each agent listens on a configured local port and accepts only paired devices with known public keys. When an agent starts, wakes, or regains network connectivity, it sends encrypted presence pings to paired peers so they can immediately mark it online.

## Pairing And Access Control

Pairing is local and short-lived. Device A shows a QR code or manual code containing a temporary address, temporary public exchange key, device name, and expiry time. Device B scans or enters the code, establishes an encrypted handshake, and both devices exchange long-term identity public keys. The pairing code expires after use.

Security rules:

- Each device generates a long-term identity key locally.
- Every peer session uses fresh session keys.
- All API traffic and file transfer traffic is encrypted and authenticated.
- Unknown peers are rejected before metadata access.
- Pairing a device does not automatically grant access to any folder.
- Access is granted per folder through explicit user action.

Each folder stores its own peer allowlist and sync policy. A paired peer can exist globally but still have no access to a given folder.

## Folder State

Each synchronized folder contains a technical `.syncer/` directory. Syncer never treats `.syncer/` as ordinary synchronized user content.

```text
.syncer/
  folder.json
  state.db
  versions/
  staging/
  locks/
```

`folder.json` stores durable folder identity and user-facing configuration: folder ID, display name, sync mode, peer allowlist, interval, folder size limit, retention policy, and conflict policy.

`state.db` stores the file index, block hashes, known peer versions, pending operations, skipped files, conflicts, transfer progress, and sync history.

`versions/` stores previous file versions according to the configured retention policy.

`staging/` stores incomplete downloads and reconstructed files before atomic replacement.

`locks/` prevents multiple agents from operating on the same folder concurrently.

## Sync Cycle

Synchronization is an explicit cycle, not constant streaming. A cycle can be triggered manually, by interval, after a peer presence ping, after network recovery, or after restarting an incomplete queue.

The cycle:

1. Scan local folder state.
2. Exchange encrypted manifests with the peer.
3. Compare local and remote file states.
4. Detect conflicts and block unsafe overwrites.
5. Build a transfer plan for safe operations.
6. Respect folder size limits before scheduling downloads.
7. Transfer files in resumable verified blocks.
8. Verify hashes before commit.
9. Move replaced versions into `.syncer/versions/`.
10. Atomically publish completed files.

Small files may use whole-file hashing. Large files use block hashes for resumable transfer and integrity checks. Full delta synchronization is not required for MVP.

## Folder Size Limits

Each folder has a configurable total synchronized data limit. The limit applies to all synchronized user files in that folder, not to individual files.

When incoming data would exceed the folder limit:

- Syncer warns the user.
- Syncer transfers only the oldest pending changes that fit.
- Remaining files are marked as pending or skipped because of the folder limit.
- The user can increase the limit or remove files to free space.
- Once space is available, skipped files can continue from the queue.

The default scheduling order for limit-constrained sync is oldest pending change first.

## Deletes And Unsynchronized Files

Deletion outside Syncer is local-only. The file is marked as missing locally, but no remote delete operation is created.

Deletion from Syncer requires an explicit choice:

- Delete only on this device.
- Delete on synchronized devices.

Files that were removed only on one side are marked as unsynchronized. If the peer is online, the UI exposes repair actions:

- Download again from peer.
- Upload again to peer.
- Keep local absence as local-only state.

## Conflict Handling

A conflict occurs when local and remote versions of the same file diverge from the same known base version. Syncer must not overwrite either side automatically.

The UI shows:

- File path.
- Local device and remote device.
- Modified timestamps.
- File sizes.
- Available versions.
- Recommended resolution actions.

Minimum conflict actions:

- Keep local version.
- Replace with remote version.
- Keep both versions as separate files.

## User Experience

Onboarding is the primary product surface.

First run:

1. Name this device.
2. Pair another device by QR code or manual code.
3. Choose a local folder.
4. Choose peer and sync direction.
5. Set optional folder limit and interval.
6. Start first sync.

Linux UI focuses on a full status panel: devices, folders, last sync, next sync, active transfers, conflicts, warnings, versions, and settings.

Android UI is task-focused: connection status, folder list, sync now, conflicts, skipped files, unsynchronized files, and notifications. It must clearly show revoked folder permissions, background restrictions, low storage, and folder limit warnings.

## Security Baseline

- No accounts, cloud service, or central server in MVP.
- Local device identity keys are generated and stored on-device.
- Handshake and transport are encrypted and authenticated.
- All peer access is allowlisted by key.
- Folder access is granted per folder.
- Network input is validated before processing.
- Manifest and transfer sizes have strict limits.
- Incomplete files are written to staging first.
- Final writes are atomic where the platform allows.
- UI never renders untrusted file names or metadata as HTML.
- Local agent API is reachable only through restricted IPC or localhost with a UI token.
- Logs must not expose encryption keys, pairing secrets, or file contents.

## Testing Strategy

MVP test coverage should include:

- Local device identity generation.
- QR/manual pairing success and expiry.
- Rejection of unknown peers.
- Encrypted presence ping.
- Manual sync.
- Periodic sync.
- Small file sync.
- Large file sync with interruption and resume.
- Hash mismatch rejection.
- Conflict detection and resolution.
- Version retention.
- Local-only delete.
- Propagated delete.
- Unsynchronized file repair actions.
- Folder size limit behavior.
- Android SAF permission loss.
- Linux agent restart during queued work.
- Offline peer and retry behavior.

## Decision Log

| # | Decision | Alternatives Considered | Reason |
|---|---|---|---|
| 1 | Build a new synchronization app, not a Syncthing wrapper. | Wrapper over Syncthing, hybrid wrapper-first model. | The goal is a new simpler product and own sync behavior. |
| 2 | MVP is for one user and their own devices. | Multi-user sharing, family/team mode. | Keeps scope focused and avoids account complexity. |
| 3 | Sync mode is per-folder, default bidirectional. | Global mode, backup-only mode. | Different folders need different behavior. |
| 4 | External deletes are local-only; in-app deletes offer local or propagated choice. | Always propagate deletes, never propagate deletes. | Prevents accidental remote loss while preserving explicit control. |
| 5 | Conflicts are shown to the user. | Newest wins, keep both automatically. | Avoids silent data loss. |
| 6 | MVP has no accounts or central server. | Local discovery server, cloud account service. | Matches private LAN/VPN use. |
| 7 | Folder metadata lives in `.syncer/`. | App-global metadata, hybrid metadata. | Keeps folder state portable and explicit. |
| 8 | Transport is encrypted from MVP. | Trust LAN/VPN only, optional encryption. | Protects users even on imperfect networks. |
| 9 | MVP scale target is medium. | Tiny prototype, large enterprise scale. | Fits personal multi-device use without overbuilding. |
| 10 | Sync is manual and periodic. | Continuous watcher sync, manual-only sync. | Better control over resources and Android battery. |
| 11 | Large file transfers are resumable by verified blocks. | Restart whole file, full delta sync. | Necessary reliability without full delta complexity. |
| 12 | Platform apps are native: Tauri Linux and Kotlin Android. | Flutter shared UI, web Linux plus native Android. | Better platform behavior and native background integration. |
| 13 | Core/protocol stack is Rust, Kotlin, Tauri. | Go core, Kotlin Multiplatform. | Strong fit for secure core and desktop integration. |
| 14 | Android uses only selected folders through SAF. | All files access, dual mode. | Safer and more compatible with Android policy. |
| 15 | Private-first project, publishable later. | Strictly private, product/store-first. | Keeps quality without app-store overhead. |
| 16 | MVP includes background service behavior. | UI-only sync, partial background mode. | Required for useful synchronization. |
| 17 | Folder versioning keeps last N versions or X days. | No versioning, overwrite/delete-only versions. | Supports recovery without complex history UI. |
| 18 | Primary UX priority is onboarding. | Conflict UX first, advanced options first. | Onboarding was the main pain point. |
| 19 | Agents send encrypted presence pings when active. | Passive online detection only. | Peers should quickly know when sync can resume. |
| 20 | Recommended architecture is Rust agent plus thin native UIs. | UI-driven sync, distributed filesystem-style engine. | Best balance for background sync, security, and MVP risk. |
| 21 | Folder access is per-folder, not global after pairing. | Pairing grants broad access. | Preserves least privilege. |
| 22 | Folder size limit applies to total synchronized data. | Per-file limit, global app limit. | Matches user intent and Android storage constraints. |
| 23 | When limit is near full, sync what fits and warn. | Fail whole sync, silently skip. | Keeps progress visible and predictable. |
| 24 | One-sided deletes are marked unsynchronized with repair actions. | Treat as conflict only, ignore silently. | Makes local-only delete behavior recoverable. |
| 25 | Limit-constrained queue uses oldest pending change first. | Smallest files first, user-priority paths first. | Most intuitive and predictable default. |

## Key Risks

- Android background execution can delay periodic sync depending on system battery policies.
- SAF access requires careful adapter design because Android folder URIs are not normal filesystem paths.
- Rust-to-Android integration adds build and packaging complexity.
- Correct conflict detection depends on durable file version lineage in `state.db`.
- Encryption, pairing, and transfer resume should be implemented with reviewed libraries rather than custom cryptographic primitives.
