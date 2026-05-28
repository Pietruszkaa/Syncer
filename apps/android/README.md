# Syncer Android App

The Android app is a Kotlin UI and platform adapter for Syncer Core.

Responsibilities:

- onboarding and pairing
- Storage Access Framework folder selection
- WorkManager periodic sync scheduling
- Foreground Service during active transfers when required
- notifications for conflicts, limits, offline peers, and permission loss
- SAF-backed file access for the Rust core

Non-responsibilities:

- direct peer protocol implementation outside Rust core
- unrestricted `All files access`
- cloud account management

The MVP must use only user-selected folders.
