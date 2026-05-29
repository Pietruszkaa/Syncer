# Android Testing

The Android layer starts as a Kotlin state and adapter scaffold. The first Android implementation step should add a Gradle wrapper, then wire these tests:

- reducer unit tests for `SyncerScreenState`
- fake `SyncerAgentBridge` tests for onboarding and sync flows
- instrumentation tests for Storage Access Framework folder selection
- WorkManager tests for periodic sync scheduling

The shared UI contract is currently validated from Node with:

```bash
npm run test:ui
```
