# Humble Redeemer 0.1.1 — prerelease

Already claimed Humble Choice games now have a clear route to Steam activation: **Humble Choice → View claimed Choice keys awaiting Steam** opens the filtered library for review. Unclaimed monthly selections remain separate. Humble claims and revealed keys do not count as Steam activation results.

Synchronization records Choice-month provenance from paid order metadata. Earlier local records remain queryable without changing their outcomes. GUI, CLI (`library --choice-only --eligible-only`) and MCP (`library` with `choice_only` and `eligible_only`) use the same filters. New batches still require review and explicit permission for unrevealed keys.

Native CI runs unit/state-machine tests, a synthetic worker/CLI/MCP integration test, GUI launch checks and installed/extracted package checks for macOS Apple Silicon/Intel, Windows x64 and Linux x64. No real account data enters CI. See the workflow and verification report for evidence.

Installers remain **unsigned by a publisher**. macOS bundles are ad-hoc signed and are not notarized. Third-party licenses accompany each installer. No telemetry or embedded account data.

**Live-service limit:** Earlier real sign-in, windowless session restoration, library/Choice synchronization and private CSV migration were verified locally. This update's live synchronization and the approved one-game activation check are pending local macOS vault authorization. Fixture success does not prove a real activation. Original history is preserved; no new claims, reveals or activations are performed merely by opening the new view.
