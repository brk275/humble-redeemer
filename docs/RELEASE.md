# Humble Redeemer 0.1.0 — prerelease

Independent native Rust app for macOS, Windows and Linux, with private local storage, human browser sign-in, reviewed redemption batches and CLI/MCP controls.

Native CI runs unit/state-machine tests, a synthetic worker/CLI/MCP integration test, GUI launch checks and packaging. These tests contain no real account data. See the workflow results and verification document for current evidence and live-test limits.

Initial installers are **unsigned by a publisher**. macOS bundles are ad-hoc signed and are not notarized. Browser drivers are provisioned separately from official sources. No telemetry or embedded account data.

Review the batch before activation. Keys reported as expired, previously activated elsewhere or exhausted cannot be made valid by this app. Ambiguous network outcomes pause for reconciliation.

**Live-service status:** This initial Rust prerelease has automated fixture coverage and native build/launch checks. Fresh real-account sign-in and a real approved activation batch are still awaiting local user verification. Preserve the legacy tool until that staged cutover passes.
