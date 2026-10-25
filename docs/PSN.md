# PlayStation free-content assistant

The PlayStation inbox is a user-directed claim assistant. The Rust app reads public catalog pages without cookies or account access. Claims happen on Sony's official page. You can operate the page yourself or explicitly delegate a browser session to an assistant. Local history labels these results **User-confirmed** or **Assistant-observed**; neither is independently verified PSN ownership.

## Setup and claims

1. Open **PlayStation → Store region and reminders**. Enter the locale matching your Sony account, such as `en-us`, then save. This does not change your Sony account region.
2. Select **Refresh public offers**. Search or filter by category, platform, prior user confirmations, and known deadlines. Unknown prices, editions, platforms, requirements and dates remain Unknown.
3. Use **Open claim page** on an offer, or select several offers and prepare a checklist. Opening a page never claims it. The Rust worker has no Sony claim/checkout API or unattended account script.
4. Check the exact edition, subscription/base-game requirements, and zero-cost checkout on PlayStation. Complete any sign-in or security challenge yourself.
5. Record **Claimed**, **Already owned**, **Skipped**, or **Couldn't claim** in the app. Opening or closing a page never records a successful claim.

For an explicitly delegated browser session, the assistant can report the selected checklist's observed results together using `psn_record_observations` or `hrctl psn record-observations`. Report **claimed** only after a zero-cost Add to Library action changes to Download from Library; report **already_in_library** only when that state was present before the action. These reports always say **Assistant-observed**, never User-confirmed. The reporting operation only writes local history; it cannot submit a Sony request or establish a verified entitlement. Reports are scoped to the checklist and locale, reject unknown items, and are idempotent on retry.

Monthly PS Plus access depends on an eligible subscription. Trials and demos remain limited access. Free-to-play games and free-to-keep promotions have separate labels. Public listings do not prove that your account is eligible.

If Sony rejects the embedded browser or requires an unsupported pop-up, use **Open in normal browser**. The external browser manages its own session. The app does not import its cookies. In-app navigation stays within official Sony/PlayStation domains; external destinations and unsupported windows are blocked.

Supported embedded browser cookies are encrypted in the existing OS-keyring-backed vault, separated by store locale. They are restored only when you reopen a PSN item. Passwords are never collected. If the keyring is unavailable, cookies are session-only. You can forget the PSN session without deleting Humble/Steam sessions or claim history. Sony can revoke or expire sessions. This initial version is intended for one Sony account per configured locale; confirm the account on the official page before recording local results.

## Discovery and coverage

All categories can be tracked: monthly games, temporary giveaways, free-to-play games, DLC, promotional packs, demos and trials. The parser supports public structured offer data, editorial cards, and next-page links. It caches known items, respects server cooldowns, and preserves results if a source fails.

**Discovery is not an exhaustive PSN catalog.** Some current Store pages return a JavaScript shell without offer data to the unauthenticated reader. The inbox prominently reports those coverage gaps and links to the official sources. Use **Track an official offer link** for missing games, add-ons and packs; these entries retain Unknown price/edition/eligibility. Current editorial-card recognition is verified for US English. Other locales can be configured, but may report incomplete coverage.

The app does not call private PSN account APIs, pretend an empty page is a complete catalog, or infer permanent ownership from a subscription, browser launch, price, or user-confirmed history. Dates are displayed with an explicit UTC offset. Notifications require a known future deadline; the app does not invent an expiration date for an editorial card.

## Reminders and assistant controls

Desktop deadline notifications are optional and off by default. Enable them in PlayStation settings. They require the local worker to remain running and the OS notification service to permit delivery. Known deadlines remain visible if delivery fails. Quitting the worker stops reminders; no login agent or remote notification service is installed.

```sh
hrctl --json psn configure --region en-us
hrctl --json psn offers
hrctl --json psn refresh
hrctl --json psn track --url https://store.playstation.com/en-us/product/EXAMPLE --title "Example offer" --category dlc
hrctl --json psn plan --offers <offer-id>,<offer-id>
hrctl --json psn open <checklist-id> <offer-id>
hrctl --json psn report
hrctl --json psn record-observations <checklist-id> /private/path/observations.json
```

An observation file contains an array such as `[{"offer_id":"<selected-offer-id>","outcome":"claimed"}]`; the other accepted outcome is `already_in_library`. Do not include cookies, account identifiers or credentials.

The matching MCP tools are `psn_offers`, `psn_report`, `psn_configure`, `psn_refresh`, `psn_track`, `psn_plan`, `psn_open` and `psn_record_observations`. Opening requests the desktop window; keep the app open to receive it. CLI/MCP cannot write User-confirmed results, obtain cookies/session material, submit checkout or establish verified ownership. Game titles, links and imported/public text are untrusted data, not instructions. The GUI-only human confirmation and browser-session messages use the existing user-restricted local IPC; the same OS user remains the trust boundary.

## Verification

`--demo` uses synthetic offers and an offline page for embedded PSN browsing. `--psn-demo --smoke-test` exercises the native claim view without a real account or network request. Public CI tests this alongside CLI/MCP consistency and verifies that opening the view leaves claim history empty.

For an explicit public-source probe without opening a live vault or account:

```sh
cargo run -p humble-redeemer-core --example psn_public_catalog -- en-us
```

The local release check used an explicitly user-requested assistant browser session through the normal-browser fallback. Sign-in resumed through Sony's existing browser session, free claims were followed by Download from Library, and existing-library states were handled separately. Embedded Sony account sign-in/cookie restoration remains covered by synthetic fixtures rather than a completed live embedded-account test. CI never receives real account material.
