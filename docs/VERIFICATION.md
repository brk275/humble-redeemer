# Verification status

## Local automated evidence

On macOS Apple Silicon, the complete Cargo workspace passes formatting, Clippy with warnings denied, and the unit/integration suite. Local synthetic WebDriver fixtures exercise verified/failed authentication, service origin restrictions, partial responses, exact ownership IDs, duplicate keys, expired/exhausted supply, distinct Steam codes, activation cooldown deadlines, cancellation, changed approval scope, interrupted Choice claims and ambiguous activation outcomes. Encryption, history preservation and secret-free snapshots are also tested.

The native worker, CLI and official-SDK MCP server pass an offline protocol integration test. All seven GUI screens render in tests. A native accessibility-driven demo selected a synthetic game, reviewed and approved its batch, completed the queue and displayed the separate result categories. The embedded native sign-in fixture was also opened on this Mac; typing and clipboard paste were visibly verified in its text input. Shared Steam cooldowns are tested across cancellation, new batches and recovery.

A RustSec dependency audit reports no known vulnerabilities in the locked dependency set after updating the MCP SDK and ZIP reader to patched releases. GTK 3's older GLib binding has an upstream iterator safety fix backported in the licensed vendored dependency; Linux CI includes an optimized regression test. See `vendor/glib-0.18.5/PATCH.md`. Transitive unmaintained-package notices remain tracked as dependency maintenance work.

## Platform and live-service limits

The GitHub Actions matrix is the source of truth for native package builds and launch checks on macOS Apple Silicon/Intel, Windows x64 and Linux x64. Public CI uses synthetic data only. Build success does not establish a live sign-in or activation.

Real Humble sign-in was completed inside the native app and independently verified by the windowless Rust worker. Steam sign-in was also independently verified, and both encrypted sessions restored without reopening a browser. A complete live library and Choice synchronization succeeded, including exact Steam ownership and exact-key reconciliation against privately imported CSV history. The original CSV files were verified unchanged. The legacy Python process was deliberately paused during its Steam cooldown, with a private backup and the cooldown carried forward to Rust.

A human-approved real activation batch is **still pending**. The inherited Steam cooldown remains enforced; successful sign-in, synchronization and fixture activation tests do not prove a real activation. No real keys or account cookies are present in public tests or packages. Desktop MCP registration and the official-SDK protocol are verified; reloading the particular desktop client remains a separate check.

Publisher signing and Apple notarization are not supplied in this prerelease. Service endpoints are unofficial and can change. Unexpected security challenges and ambiguous requests require user action; the application does not bypass them.
