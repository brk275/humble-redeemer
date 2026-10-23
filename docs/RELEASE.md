# Humble Redeemer 0.1.0 — prerelease

Independent native Rust app for macOS, Windows and Linux, with private local storage, human browser sign-in, reviewed redemption batches and CLI/MCP controls.

Native CI runs unit/state-machine tests, a synthetic worker/CLI/MCP integration test, GUI launch checks and packaging. These tests contain no real account data. See the workflow results and verification document for current evidence and live-test limits.

Initial installers are **unsigned by a publisher**. macOS bundles are ad-hoc signed and are not notarized. Browser drivers are provisioned separately from official sources. No telemetry or embedded account data.

Review the batch before activation. Keys reported as expired, previously activated elsewhere or exhausted cannot be made valid by this app. Ambiguous network outcomes pause for reconciliation.

**Live-service status:** Real Humble sign-in inside the native app, windowless Humble/Steam session restoration, live library/Choice synchronization, exact ownership and private CSV migration have been verified locally. The Python worker is preserved and deliberately paused, with its Steam cooldown carried forward. A real approved activation batch is still pending; fixture success is not a live activation result. See the verification report for platform checks and remaining limits.
