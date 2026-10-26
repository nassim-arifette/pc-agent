# Tool context measurements and compaction

The offline `pcagent-eval context-benchmark <scenario> --anonymize --out <new-file>` command projects
one immutable capture through the same privacy and citation machinery, then measures UTF-8 bytes
before and after context compaction. It does not contact a provider or collect new measurements.

`ToolContext` replaces an exactly repeated result with `same_result_as`, retaining the new citation
IDs and pointing to the original result. Equality excludes bookkeeping IDs only. Changed values,
timestamps, windows, sources and coverage are significant; a fresh read is never treated as a
duplicate just because its numeric values happen to match. Collectors still run normally.

Once older tool context exceeds 32 KiB, old replies omit metadata only where it is identical to the
nearest enclosing record. An explicit inheritance note explains the resulting structure. The newest
reply stays complete. Values, units, entities, coverage, evidence and citations remain available;
the local verifier receives the complete original projection, not the compact model transcript.
Missing measurements remain null. Added `readable` fields accompany raw values without replacing
them. The final privacy filter runs after fresh findings and citation bookkeeping have been inserted.

On the live 96-call capture from 3 October (`smoke.json`, SHA-256
`fd88d433400c30f9bae713d5f1c89741f8d3b44c834afe2da577c2c5bed55240`), the initial benchmark measured
1,136,274 full projection bytes versus 865,304 compact bytes, and 64,658,614 versus 49,273,037 bytes
of cumulative tool context: about 24% less. Only one result was exactly repeated. These numbers
exclude system messages and tool definitions; they are neither token counts nor billed cost savings
nor a quality score. Newer runs retain their own registry hash and output file rather than replacing
this baseline.

The final Windows release rerun on 4 October, using the same unchanged capture, measured
1,136,940 full projection bytes versus 865,970 compact bytes, and 64,695,008 versus 49,309,431
cumulative tool-context bytes (about 24% less). It retained 96 calls and one exactly repeated
result. Its current registry SHA-256 is
`6e509293433dc3dd02b8140ef2ce1df9337df4f57e8bb4aaa8746dbe06afc252`; the changed projection totals
are kept separately from the initial benchmark and frozen model-reference registry. This was
an offline projection check, not a new provider run or a scored comparison.

Tests exercise changed measurements, freshness, inherited metadata, raw-unit formatting, citation
preservation, complete/anonymized names and immutable replay captures. Real answer-quality
regression assessment remains part of the independently graded evaluation campaign.

## Declared tools: the capability registry

A turn declares to the model only the tools that work on this PC and in this turn
(`crates/agent/src/capabilities.rs`). The host detects a closed set of capabilities when the turn
starts, with probes it already runs, and one table (`REQUIREMENTS`) maps every tool to the
capabilities it needs. An absent tool is not declared: the model cannot see or call it (an
undeclared call is rejected), and neither the system prompt nor another tool's text names it.
Available means that the capability exists here and now, not that data already exists:
`get_approved_elevated_inspection` stays declared in the desktop app without any approved result,
because its `needs_admin` reply is what makes the chat offer the check to the person.

| Capability | Present when | Probe |
| --- | --- | --- |
| `windows` | the native Windows collectors are built (other systems only build stubs) | `cfg!(windows)` |
| `history_sampler` | the turn has the persistent local sampler | the toolbox's sampler |
| `storage_scopes` | a storage scope is approved for this turn | the turn's approved scopes |
| `elevated_helper` | the host runs locally approved helper tasks and `pcagent-elevated.exe` is installed next to it | the desktop app checks the file once at start; the CLI and the evaluation have no helper flow |
| `web_search` | cloud mode, the web search consent and a saved Tavily key (C6) | the toolbox's web lookups |
| `action_proposals` | the action journal opened | the toolbox's action desk |
| `nvidia_gpu` | NVML lists an NVIDIA GPU | the NVML devices of the latest snapshot |
| `battery` | Windows lists a system battery, readable or not | the Live PC battery read (desktop app only) |
| `ntfs_volumes` | a fixed NTFS volume exists | the file system of the live volumes (desktop app only) |

A hardware probe that has not run yet counts as present: the tool then reports a missing device
itself, while hiding a working tool would cost an answer.

| Tools | Need |
| --- | --- |
| `get_system_snapshot`, `get_processes`, `get_process_details`, `get_process_tree`, `get_process_activity` and the ten inspections of `system_tools.rs` | `windows` |
| `list_storage_scopes`, `get_storage_overview`, `start_storage_scan`, `get_largest_items`, `get_cleanup_candidates` | `storage_scopes` |
| `get_user_preferences`, `get_selected_evidence` | nothing |
| `get_metric_history`, `get_activity_events`, `record_activity` | `windows`, `history_sampler` |
| `get_approved_elevated_inspection` | `windows`, `elevated_helper` |
| `prepare_action_plan` | `windows`, `action_proposals` |
| `lookup_driver_notes`, `lookup_known_issue`, `lookup_upgrade_price`, `lookup_pc_specs` | `windows`, `web_search` |

Hardware capabilities change texts, not the list (`VARIANTS`, constant variants with the same
name, class and argument names). Without `nvidia_gpu`, `get_system_snapshot` says that no NVIDIA
reading is measured; without `battery`, `get_power_and_thermal` says there is no battery to read
and `get_approved_elevated_inspection` no longer offers `battery_energy`; without `ntfs_volumes` it
no longer offers `mft_inventory`. That tool never offers `process_images` or `performance_trace`,
which no host prepares. Without `storage_scopes`, `prepare_action_plan` offers no action on a
scanned item and names no storage tool. The system prompt names `get_approved_elevated_inspection`
only when the turn declares it, and the energy used on battery only with its task; the
`action_refs` description of `submit_answer` names `prepare_action_plan` only when it is declared.
The answer schema itself (C1) never changes.

The desktop app restricts every turn with its probes (Investigate copies the restricted list; Explain
declares only `get_selected_evidence`). The CLI restricts `diagnose`, `chat` and `inspect`, without
a helper flow or web lookups; `diagnose --preview-cloud` lists the tools a run would declare, and
`inspect` names the capability an unavailable tool lacks. The evaluation restricts its live model
capture, whose scenario keeps the declared list for its replays, and its live MCP server. A toolbox
that is never restricted (`Workstation::offer_available` not called) declares the whole catalog, as
replays and capture sweeps expect.

The global registry identity (`registered_tool_registry_sha256`, which calibrations check) is
computed from the fixed canonical catalog of `Workstation::tool_specs` with `prepare_action_plan`,
`get_selected_evidence` and the canonical `submit_answer` definition, never from a turn's
restricted list; replays keep each scenario's own tool list. A new tool needs a row in the table:
a test fails without one, and an unlisted tool is never declared.
