# Scenarios

Only synthetic or explicitly reviewed anonymized scenarios belong here. Raw captures and results
belong outside the repository, for example `D:\pcagent-tmp\captures`.

The five checked-in JSON files are synthetic harness fixtures: memory pressure on an 8 GiB PC,
busy CPU, disk latency, a moderate core-metric sample and a nearly full volume. They contain no
real process identities or user data and are not benchmark results. Regenerate them with
`pwsh -File eval/generate-fixtures.ps1`. Replay preserves their `synthetic_fixture` origin.

The real campaign from the plan still needs these situations:

| Situation | Evidence to capture or acknowledge as missing |
| --- | --- |
| Browser tabs on limited RAM | System memory, process working/private memory, paging |
| Application memory growing over ten minutes | Time series and stable process identity |
| Whether 8 GiB is sufficient | Representative workload and sustained pressure, not capacity alone |
| Defender scan | Process activity and corroborating scan state |
| Windows Update | Process activity, update state and events |
| OneDrive sync | Process activity and observed sync state |
| Excessive startup programs | Enabled startup inventory and activity |
| Nearly full C: | Volume capacity and authorized folder scan |
| Downloads and caches | Item sizes, classifications and safe review guidance |
| Game stutter | Measurements during stutter, GPU/VRAM and CPU activity |
| Hot laptop on battery | Power state, thermal and throttling evidence |
| Slow disk | Latency, queue and workload over time |
| Application crash | Relevant bounded event history |
| Long-pending restart | System state and dated evidence |
| PC without an observed problem | Representative moderate readings and explicit coverage limits |

Several of these need tools from work packages 4, 5 and 9. Their absence is a coverage gap, not
an invitation to fabricate a successful capture. See [the harness guide](../../docs/eval.md).
