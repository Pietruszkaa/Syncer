# Syncer

Syncer is a private-first file synchronization application for one user's Linux and Android devices over LAN or VPN.

The MVP uses:

- Rust core and local agent
- Tauri Linux desktop UI
- Kotlin Android UI
- local pairing without accounts
- encrypted peer-to-peer transport
- per-folder sync configuration

The product design is documented in [docs/syncer-design.md](docs/syncer-design.md).

## Current Workspace

```bash
cargo test --workspace
cargo run -p syncer-agent -- --help
```
