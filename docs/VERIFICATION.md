# Verification status

## Local automated evidence

On macOS Apple Silicon, the complete Cargo workspace passes formatting, Clippy with warnings denied, and the unit/integration suite. Local synthetic WebDriver fixtures exercise verified/failed authentication, service origin restrictions, partial responses, exact ownership IDs, duplicate keys, expired/exhausted supply, distinct Steam codes, activation cooldown deadlines, cancellation, changed approval scope, interrupted Choice claims and ambiguous activation outcomes. Encryption, history preservation and secret-free snapshots are also tested.

The native worker, CLI and official-SDK MCP server pass an offline protocol integration test. All seven GUI screens render in tests. A native accessibility-driven demo selected a synthetic game, reviewed and approved its batch, completed the queue and displayed the separate result categories. The embedded native sign-in fixture was also opened on this Mac; typing and clipboard paste were visibly verified in its text input. Shared Steam cooldowns are tested across cancellation, new batches and recovery.

A RustSec dependency audit reports no known vulnerabilities in the locked dependency set after updating the MCP SDK and ZIP reader to patched releases. Transitive unmaintained-package notices remain tracked as dependency maintenance work.

## Platform and live-service limits

The GitHub Actions matrix is the source of truth for native package builds and launch checks on macOS Apple Silicon/Intel, Windows x64 and Linux x64. Public CI uses synthetic data only. Build success does not establish a live sign-in or activation.

A real Steam connection was verified with the compatibility browser adapter during development. Fresh Humble sign-in and a human-approved real batch have **not yet been verified for this Rust version**; the new embedded-to-windowless session path is undergoing local user verification. The legacy Python worker remains preserved. Cutover requires it to finish or be deliberately paused, local history migration, fresh provider sign-in and an explicitly reviewed batch. No real keys or account cookies are present in public tests or packages.

Publisher signing and Apple notarization are not supplied in this prerelease. Service endpoints are unofficial and can change. Unexpected security challenges and ambiguous requests require user action; the application does not bypass them.
