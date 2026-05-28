# ADR 0001: Rust Agent With Thin Platform UIs

## Status

Accepted

## Context

Syncer must run on Linux and Android, synchronize files in the background, encrypt peer-to-peer traffic, resume transfers, retain versions, and expose a simple onboarding-first UI.

## Decision

Use a Rust Syncer Core agent as the synchronization authority on every device. Linux uses a `systemd --user` service and Tauri UI. Android uses Kotlin UI with Rust bindings through UniFFI or JNI.

## Consequences

- Synchronization behavior stays consistent across platforms.
- UIs remain focused on onboarding, state display, and user decisions.
- Android integration needs a dedicated Storage Access Framework adapter.
- Rust-to-Android build complexity is accepted because sync correctness and security benefit from a shared core.
