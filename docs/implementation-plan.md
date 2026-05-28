# Syncer Implementation Plan

## Milestone 1: Core Domain And Agent Foundation

- Rust workspace with `syncer-core` and `syncer-agent`.
- Validated IDs, names, relative paths, folder limits, file presence states, pairing tickets, and presence messages.
- CLI support for local device initialization and folder-limit planning.
- Unit tests for path safety and folder-limit scheduling.

## Milestone 2: Persistent Folder State

- SQLite-backed `state.db`.
- `folder.json` read/write with schema versioning.
- File scanner for native Linux paths.
- Android storage adapter contract for SAF URIs.
- Persistent queue for transfers, skipped files, conflicts, and unsynchronized files.

## Milestone 3: Secure Peer Protocol

- Device identity key storage.
- Local QR/manual pairing ticket generation.
- Authenticated encrypted handshake.
- Encrypted presence ping.
- Peer allowlist enforcement before metadata exchange.
- Manifest exchange with strict size limits.

## Milestone 4: Transfer Engine

- Blocked transfer sessions.
- Resume from last verified block.
- Hash verification before commit.
- Staging writes and atomic publish.
- Folder size-limit enforcement.
- Version retention in `.syncer/versions`.

## Milestone 5: Platform Integration

- Linux `systemd --user` service installer.
- Tauri UI over restricted local agent IPC.
- Android Kotlin UI with UniFFI/JNI bindings.
- WorkManager periodic sync.
- Foreground Service for active transfers.
- Android notification and SAF permission recovery flows.

## Milestone 6: End-To-End MVP

- Pair Linux and Android.
- Configure a bidirectional folder.
- Run manual and periodic sync.
- Handle conflicts, one-sided deletes, folder limit warnings, and peer offline state.
- Ship private Linux build and Android APK.
