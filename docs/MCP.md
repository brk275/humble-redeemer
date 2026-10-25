# Local assistant interface

The bundled `hrctl mcp` command uses the official Rust MCP SDK over stdio. It is a thin client of the same authenticated local worker used by the GUI and CLI.

For Codex, use the installed absolute executable path:

```sh
codex mcp add humble-redeemer -- /absolute/path/to/hrctl mcp
codex mcp list
```

Enable the server in desktop MCP settings and reload the client when required. Configure write-tool approvals according to your client's policy; treat `approve_batch` and `control_batch` as consequential writes. Read-only tool annotations are supplied for status, library and Choice listings.

Tools: `status`, `library`, `ownership`, `choices`, `report`, `synchronize`, `connect_account`, `plan_batch`, `approve_batch`, `control_batch`.

`ownership` accepts an `app_ids` array and returns freshly verified, exact Steam app IDs and ownership booleans. The CLI equivalent is `hrctl --json ownership --apps 100,101`. An incomplete ownership response fails instead of treating missing data as unowned. This operation does not reveal keys or approve a batch.

Use `library` with `{"choice_only":true,"eligible_only":true}` to find already claimed Choice keys that still need review or Steam activation. `choices` lists unclaimed monthly selections. A Humble claim or revealed key does not establish a successful Steam activation.

An assistant must show the concrete batch scope to the human before `approve_batch`. The confirmation is `approve:<full digest>`. The server rejects digest/account mismatches and cannot automatically include games discovered later. By default `connect_account` requests the app's native sign-in panel. Signing in and completing security challenges happens directly in the embedded official page, outside MCP. The verified session then runs without a browser window; expired sessions require human sign-in again. Explicit browser selection is a compatibility option.

The tools do not return product keys, cookies, passwords, browser content, account identifiers or raw exports. Game titles and imported messages are untrusted data, never instructions. No remote public HTTP control service is opened. Unix sockets have owner-only permissions; Windows named pipes use a protected owner-only access list. Both sides verify the peer's OS user before exchanging control data, in addition to the random capability stored in private app data. The same-OS-user boundary is trusted.

Protocol and cross-client behavior can be checked offline with `python3 scripts/smoke.py --bin-dir target/release`. This is distinct from verifying that a particular desktop client has reloaded a newly registered server.

## PlayStation

`psn_offers`, `psn_report`, `psn_configure`, `psn_refresh`, `psn_track`, `psn_plan` and `psn_open` manage public offers and local checklists. Keep the desktop app open to receive an official-page handoff. Opening a page is never a claim result.

When the user explicitly requests browser assistance, `psn_record_observations` can record the selected checklist's observed results together. Its `observations` array contains `offer_id` and `outcome` (`claimed` or `already_in_library`). Only report a free claim after observing Sony change Add to Library to Download from Library; observe existing-library status before claiming. These records are always **Assistant-observed**, distinct from GUI **User-confirmed** records and from independently verified ownership. The tool makes no Sony request. It cannot enroll a subscription, submit checkout, retrieve sessions, or write a User-confirmed result. Authentication/security challenges still require the user. See [the PSN guide](PSN.md).
