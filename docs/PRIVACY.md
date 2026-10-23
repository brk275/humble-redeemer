# Private data boundary

The repository contains source, synthetic tests and documentation only. Production state lives in platform user-data storage; demo state uses a different profile. `HUMBLE_REDEEMER_DATA_DIR` is an explicit local override, intended for isolated test profiles and managed installations.

SQLite stores library metadata, statuses, explanations, batches and cooldowns. Session cookies, order access data and product keys are separate authenticated-encryption records. A random 256-bit vault key is protected by macOS Keychain, Windows Credential Manager or Linux Secret Service. ChaCha20-Poly1305 encrypts each secret record using a fresh nonce. If the system store cannot be used, new secrets remain in memory and are not written to SQLite. Restarting then requires reconnecting and resynchronizing.

Account passwords are never collected by the application. Official browser pages handle passwords, Steam Guard and security challenges. Private browsing is enabled to reduce browser persistence. Browser profiles are isolated in a private application directory and cleaned up when the helper exits normally. An application or OS crash can leave temporary browser files on disk; this is not a secure-erasure guarantee.

Keys are masked until explicitly shown locally. Raw key exports require a separate explicit option and are saved to the user-selected path. Redacted CSV reports omit keys and neutralize spreadsheet formulas. Original legacy CSVs are never modified; ambiguous owned/used results remain ambiguous after import. Python pickle cookie files are never imported.

The app has no telemetry, cloud synchronization or automatic crash upload. Provider errors are mapped to fixed explanations; diagnostics redact key patterns, tokens, email addresses and home-directory prefixes. MCP has no secret-retrieval or raw-key-export tool.

Driver downloads use official HTTPS sources. Mozilla release SHA-256 digests are required; Google archives are checked against the Google Storage MD5 integrity header or official object-metadata API over TLS. Metadata fallbacks must match the exact object name and archive size. Extracted binaries receive a local SHA-256 receipt that is checked before reuse. Google's storage checksum is an integrity check, not a publisher signature. Executable updates are not silently installed by a general-purpose self-updater.

Before publication run `python3 scripts/audit.py`, and locally pass `--private-history` to compare staged source and Git history against known private legacy values without printing those values. Build packages from a clean checkout; never package application-data directories. Signing credentials belong only in a supported secret store or CI secrets, never source.
