# `pcagent-policy` — cleanup policy (spec 10.1–10.3, 11.1, 12.1, 13.3, 15.2, 16.1, 17.1)

Pure, deterministic Rust. The crate models **Windows** paths but performs no filesystem access and executes nothing, so every rule is unit-tested on Linux and Windows CI alike. The only environment access is `KnownFolders::from_env()` / `Policy::from_env()`; tests inject values with `KnownFolders::from_lookup`.

## Modules

| Module | Responsibility |
| --- | --- |
| `path` | `NormalizedPath::parse(&str) -> Result<NormalizedPath, PathRefusal>`: case-insensitive comparison keys, `/` → `\`, `\\?\` and `\\?\UNC\` prefixes, duplicate separators, `.` segments, Win32 trailing dot/space stripping. Refuses: relative and drive-relative paths, `\\.\` / `\??\` device namespace, `..` and dot-only segments, `:` after the drive (alternate data streams), 8.3-shaped short names (`PROGRA~1`), reserved device names, invalid/control characters, trailing dots/spaces under `\\?\` (they name a different file there), over-long paths. UNC paths parse but are `is_network()`. |
| `protect` | Protected locations and `check_target`. Built-in patterns apply to every drive; `KnownFolders` adds relocated `%WINDIR%`, `%ProgramFiles*%`, `%ProgramData%`, `%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%`, `%TEMP%`. |
| `prefs` | `Preferences` (schema_version 1, serde, `deny_unknown_fields`): excluded paths and categories, preserved apps, permissions. `view()` is the model-safe `get_user_preferences` output. |
| `classify` | `CandidateInput` → `Classification` (10.1 table). |
| `adapters` | Declarative adapter registry (11.1). |
| `plan` | `build_plan` (10.2/10.3), `cleanup_candidates` (`get_cleanup_candidates`) and path-free model views. |

## Protection rules

- **Subtree** (the path, its descendants and its ancestors are forbidden): `Windows` (with specific kinds for `Installer`, `WinSxS`, `System32`), `Windows.old`, `$WINDOWS.~BT/~WS`, `pagefile.sys`, `swapfile.sys`, `hiberfil.sys`, `System Volume Information`, `$Recycle.Bin`, `Recovery`, `$WinREAgent`, `Boot`, `bootmgr`, `EFI`, `Documents and Settings`, `Program Files*` (uninstall guidance only).
- **Root only** (the root and its ancestors are forbidden, descendants are evaluated further): drive roots, `Users`, each profile root, `AppData`, `AppData\Local`, `LocalLow`, `Roaming`, `ProgramData` (descendants are recommendation-only).
- `AppData\Local\Temp` itself is recommendation-only; a temporary folder is never automatically a target.
- Network paths, user-excluded paths (and their ancestors) and unparseable paths are forbidden.
- Forbidden system items carry an `OfficialTool` (Storage Sense, Disk Cleanup, Installed apps, virtual memory, power options, System Protection).

## Classification

Order: path refusal → protection/exclusion → adapter match → attributes (reparse point, cloud placeholder, system → forbidden; hidden → extra condition) → excluded category → name/extension heuristics. Only a registered adapter can produce `regenerable_identified`. Folder names (`node_modules`, `target`, `Temp`, …) only yield recommendations with conditions. Personal data (documents, media, projects, games, ML models, VMs, archives, user folders) is inspect or guided move only. Justifications and conditions are fixed `&'static str` templates: file names never flow into them, so a file named like an instruction stays plain data.

## Adapters

Exactly one executable adapter: `pcagent.synthetic_demo_cache`, targeting `%LOCALAPPDATA%\pcagent\synthetic-demo-cache`, marked `synthetic: true`. Browser (Chrome, Edge), npm, Cargo and pip caches are declared `validated: false`, `RecommendOnly`.

## Plans

- `estimated_bytes` = observed allocation or `None`; logical size is reported separately and never used as a reclaim estimate.
- Recycle bin / same-volume quarantine: `immediate_reclaim_bytes = 0`, `deferred_reclaim_bytes = allocation`.
- Allowed set = validated adapter targets and Downloads installers (recycle, never preselected). Everything else is a recommendation or refused.
- Duplicate spellings and actionable children of actionable parents are deduplicated; an actionable parent containing any non-actionable candidate is demoted.
- `goal_status`: `reached` | `partial { reachable_bytes }` | `not_reachable`. Personal or system data is never added to reach a goal.
- At most `MAX_PLAN_CANDIDATES` (5,000) inputs; the remainder is reported as truncated.

## Safety boundaries

- The model receives only `PreferencesView`, `CleanupCandidatesView` and `PlanView`: opaque refs, classes, verdicts and byte counts. No paths, executable names or user notes.
- `item_ref` must be opaque (not derived from a file name) — the scanner that produces `CandidateInput` owns this.
- A plan is not an authorization; executors must revalidate identity, reparse points and exclusions immediately before operating (spec 11.3). Nothing in this crate executes.

## Not validated on real Windows

- Normalization is a textual model of Win32 rules. It has not been compared against `GetFullPathNameW`, NTFS case tables (non-ASCII case folding uses Rust `to_lowercase`) or real short-name generation.
- Known-folder detection reads environment variables, not `SHGetKnownFolderPath`; redirected folders (e.g. OneDrive-backed Documents) are not detected.
- Adapter targets and cache layouts (Chrome, Edge, npm, Cargo, pip) are based on default documented layouts, not observed on a test PC.
- Attribute flags are trusted from the caller; no native attribute reading or reparse-point resolution exists here.
