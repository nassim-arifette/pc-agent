# PC Agent

A read-only, CLI-first Windows investigation agent. Initial implementation based on **PC Agent specification v0.1 (2026-09-25), Lot A**. This is an early prototype, not a finished optimizer or a validated hackathon submission.

## Scope of this first slice

- Rust workspace with a thin CLI, shared contracts, Windows telemetry, deterministic diagnostics and a Nebius adapter.
- `snapshot`, `processes`, `doctor` and `diagnose` commands.
- Explicit missing-data statuses, source identifiers, observation windows and session-scoped process identities.
- No file deletion, application termination, elevation, startup changes, scanning of documents or automatic cloud requests.

Implementation and validation details will be added with the first code commit. No performance gain or provider integration is claimed as validated.

## License

No open-source license has been selected yet. Keep this repository private until the license and submission requirements have been reviewed.
