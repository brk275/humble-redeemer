# Security reporting

Do not post keys, cookies, credentials, account identifiers, browser profiles, raw exports or unredacted logs in an issue. Reproduce with demo data where possible. Use GitHub's private vulnerability reporting if enabled for a security-sensitive report.

The local OS account and its unlocked credential store are trusted. An attacker with control of that account can access application state and local IPC. There is no public network control listener. Never enable a generic GUI inspection port against a real account profile.
