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
