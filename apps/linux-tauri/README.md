# Syncer Linux UI

The Linux app is a Tauri control panel for the local `syncer-agent`.

Responsibilities:

- first-run onboarding
- device pairing
- folder setup
- sync status
- conflict resolution
- skipped and unsynchronized file actions
- settings for interval, limits, and retention

Non-responsibilities:

- direct file synchronization
- direct peer protocol handling
- key exchange
- version retention

The UI communicates with the local agent through restricted IPC or localhost with a per-install token.

## Current Scaffold

- `src/index.html` is the first dashboard surface.
- `src/app.js` renders a local snapshot and is ready to swap in a Tauri command adapter.
- `src/agent-adapter.js` defines the Tauri IPC command boundary and local development fallback.
- `src/view-model.js` maps shared contract data into Linux dashboard state.
- `src-tauri` provides the native Linux shell and restricted IPC commands for `syncer-agent`.
- `test/*.test.js` validates the adapter and view model with Node's built-in test runner.

## Local Agent Runtime

The Tauri shell reads its local state from the app data directory:

- `state.json` stores selected folders, peer endpoints, limits, and sync intervals.
- `device.json` is the default local device identity used by `syncer-agent`.

Runtime overrides:

- `SYNCER_AGENT_BIN` points to a custom `syncer-agent` binary.
- `SYNCER_DEVICE_CONFIG` points to a custom device config path.

Development:

- `npm run test:ui` validates the shared UI contract and Linux adapter.
- `cargo test --manifest-path apps/linux-tauri/src-tauri/Cargo.toml` validates the Tauri command bridge.
- `cd apps/linux-tauri && npm run dev` starts Tauri and a static local server for `src/`.
