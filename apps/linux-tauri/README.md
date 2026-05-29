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
- `test/*.test.js` validates the adapter and view model with Node's built-in test runner.
