# Humble Redeemer 0.2.0 — PSN assistant prerelease

The new **PlayStation** inbox tracks public free-content offers by store locale, with category/platform filters, user-operated claim checklists, local history and optional deadline reminders. Official pages open inside the app, with a normal-browser fallback. Supported embedded cookies use the encrypted local vault; passwords stay on Sony's pages.

You complete sign-in and Add to Library yourself. Records are explicitly **User-confirmed**. CLI/MCP can query offers, refresh public sources, track official links, prepare checklists and request a claim window; they cannot confirm ownership, read sessions, submit checkout or enroll subscriptions.

**Coverage limit:** Discovery is not an exhaustive Store catalog. Some current public Store pages return only a JavaScript shell. The app preserves cached data, shows coverage gaps and Unknown fields, and accepts manual official offer links for missing games, DLC and packs. US English editorial recognition is the initial live-tested catalog path. Subscription games, limited trials, demos and permanent giveaways have distinct labels. See [PSN usage and limitations](PSN.md).

Humble/Steam records and approved-batch rules are preserved. All public tests and CI use synthetic data. See [verification status](VERIFICATION.md) for automated platform evidence and the separate local live-claim acceptance result.

Installers remain **unsigned by a publisher**; macOS bundles are ad-hoc signed, not notarized. Checksums and dependency license notices accompany the packages. No telemetry or real account material is included.
