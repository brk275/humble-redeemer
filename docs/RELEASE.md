# Humble Redeemer 0.2.0 — PSN assistant prerelease

The new **PlayStation** inbox tracks public free-content offers by store locale, with category/platform filters, user-operated claim checklists, local history and optional deadline reminders. Official pages open inside the app, with a normal-browser fallback. Supported embedded cookies use the encrypted local vault; passwords stay on Sony's pages.

Open an individual claim page directly or prepare a multi-item checklist. Operate Sony's page yourself, or explicitly delegate a browser session to an assistant. Local history distinguishes **User-confirmed** and **Assistant-observed** results. CLI/MCP can query offers, manage checklists and report scoped observations; they cannot establish verified ownership, read sessions, submit checkout or enroll subscriptions.

**Coverage limit:** Discovery is not an exhaustive Store catalog. Some current public Store pages return only a JavaScript shell. The app preserves cached data, shows coverage gaps and Unknown fields, and accepts manual official offer links for missing games, DLC and packs. US English editorial recognition is the initial live-tested catalog path. Subscription games, limited trials, demos and permanent giveaways have distinct labels. See [PSN usage and limitations](https://github.com/brk275/humble-redeemer/blob/v0.2.0/docs/PSN.md).

Humble/Steam records and approved-batch rules are preserved. All public tests and CI use synthetic data. See [verification status](https://github.com/brk275/humble-redeemer/blob/v0.2.0/docs/VERIFICATION.md) for platform evidence and the separately verified live assisted-browser claims. Live embedded Sony sign-in/cookie restoration remains unverified; native restoration has synthetic coverage.

Installers remain **unsigned by a publisher**; macOS bundles are ad-hoc signed, not notarized. Checksums and dependency license notices accompany the packages. No telemetry or real account material is included.

Tagged CI builds stage a draft prerelease. Publication follows inspection of the final extracted installers and a private comparison against known sensitive values; CI never receives those private values.
