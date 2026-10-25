# Verification status

## Local automated evidence

On macOS Apple Silicon, the complete Cargo workspace passes formatting, Clippy with warnings denied, and the unit/integration suite. Local synthetic WebDriver fixtures exercise verified/failed authentication, service origin restrictions, partial responses, exact ownership IDs, duplicate keys, expired/exhausted supply, distinct Steam codes, activation cooldown deadlines, cancellation, changed approval scope, interrupted Choice claims and ambiguous activation outcomes. Encryption, history preservation and secret-free snapshots are also tested.

The native worker, CLI and official-SDK MCP server pass an offline protocol integration test. All eight GUI screens render in tests. A native accessibility-driven demo selected a synthetic game, reviewed and approved its batch, completed the queue and displayed the separate result categories. The embedded native sign-in fixture was also opened on this Mac; typing and clipboard paste were visibly verified in its text input. Shared Steam cooldowns are tested across cancellation, new batches and recovery.

Choice regressions cover fully claimed monthly orders whose keys remain Ready or Unrevealed, explicit reveal permission, and older local records without Choice provenance. CLI and MCP return the same claimed-Choice filter results. Opening that view does not select, approve, reveal or activate games.

Queue regressions cover stale paused approvals, expired cooldown displays, and completed history after an account connection changes. Stale batches offer a fresh review of only the remaining eligible items; creating that review does not approve or start it. Completed queues show individual outcome totals rather than treating every processed key as redeemed.

Session regressions use a synthetic local HTTPS server with normal certificate validation to exercise Steam's two-step renewal, host-scoped cookies, token rotation and restoration after a restart. Tests cover path/expiry preservation, cookie deletion, renewal destination restrictions, temporary authentication failures and preservation of batch scope for the same account. The native Steam handoff has a cookie-based fallback when the embedded page's JavaScript probe cannot report success. These automated checks do not establish that a real Steam session has been saved or renewed; that requires a local account check.

A RustSec dependency audit reports no known vulnerabilities in the locked dependency set after updating the MCP SDK and ZIP reader to patched releases. GTK 3's older GLib binding has an upstream iterator safety fix backported in the licensed vendored dependency; Linux CI includes an optimized regression test. See `vendor/glib-0.18.5/PATCH.md`. Transitive unmaintained-package notices remain tracked as dependency maintenance work.

## Platform and live-service limits

The GitHub Actions matrix is the source of truth for native package builds and launch checks on macOS Apple Silicon/Intel, Windows x64 and Linux x64. Public CI uses synthetic data only. Build success does not establish a live sign-in or activation.

Real Humble sign-in was completed inside the native app and independently verified by the windowless Rust worker. Steam sign-in was also independently verified, and both encrypted sessions restored without reopening a browser. A complete live library and Choice synchronization succeeded, including exact Steam ownership and exact-key reconciliation against privately imported CSV history. The original CSV files were verified unchanged. The legacy Python process was deliberately paused during its Steam cooldown, with a private backup and the cooldown carried forward to Rust.

A separately reviewed local Choice batch completed real Humble key reveals and Steam activations on this Mac. Successful activations were independently verified by a fresh exact Steam ownership lookup. Humble supply exhaustion and rejected reveal requests remained separate blocked outcomes. Public tests and reports contain no real keys, account identifiers or cookies; private service evidence stays outside the repository. These local results do not establish live-service compatibility on the other platforms.

Version 0.1.2 has locally restored both real account connections after a complete macOS app and worker restart, without another service login. A fresh real Steam sign-in closed the embedded panel automatically; its encrypted renewal cookie was also restored after a full restart. The Steam verifier accepts the current store page's account number only when it matches the complete SteamID64 in the session; the older page format remains covered. Silent renewal has been tested with synthetic HTTPS fixtures and has not been forced against a real account. Desktop MCP registration and the official-SDK protocol are verified; reloading the particular desktop client remains a separate check.

Publisher signing and Apple notarization are not supplied in this prerelease. Service endpoints are unofficial and can change. Unexpected security challenges and ambiguous requests require user action; the application does not bypass them.

## PlayStation 0.2.0 acceptance

Synthetic tests cover public prices, region/edition distinctions, monthly-versus-catalog classification, pagination, deduplication, expired items, cooldown persistence, encrypted/session-only browser material, and user-confirmed history after restart. CLI and MCP expose the same PSN state; neither can confirm a claim. A native offline claim-page fixture opens on macOS without recording ownership. All eight GUI screens render without credentials.

The explicit US public-source probe found usable monthly/free-to-play editorial cards and coverage gaps on several Store catalog sources returning JavaScript shells. Public-source data is a discovery lead, not account eligibility or an exhaustive catalog. Live user-operated PSN claim acceptance and PSN cookie restoration are pending. The prerelease must not be published until the human completes that local acceptance step. Other-platform launch verification is tracked by the native CI matrix and is separate from a live Sony account check.
