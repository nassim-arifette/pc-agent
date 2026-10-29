# Web lookups (chantier 15, contract C6)

PC Agent can check a few facts on the web that no measurement of the PC can give:
- a driver maker's notes for an installed driver;
- a known issue behind a Windows update number or a stop code;
- the price of a compatible memory module;
- the PC model's specification sheet.

Search is done by [Tavily](https://tavily.com) and is optional. Without a key, or without the
person's consent, the tools are not declared and everything else works as before.

## Tools

The model chooses a lookup and a measured reference. It never writes the query: the query is
composed on the PC from local measurements ([`external_tools.rs`](../crates/tools/src/external_tools.rs)).

| Tool | Argument | Query composed from | Sites searched |
| --- | --- | --- | --- |
| `lookup_driver_notes` | `device_ref` from `get_drivers` in the same turn | driver provider, device model, installed version | the chip or driver maker (nvidia.com, intel.com, amd.com, realtek.com…), else the PC maker |
| `lookup_known_issue` | `kb` (`KB` + 6 to 8 digits) or `code` (`0x` + up to 8 hex digits) | that number only | learn.microsoft.com, support.microsoft.com |
| `lookup_upgrade_price` | `component`: `memory` | measured memory type, form factor, speed and module size | shops and comparators of the Windows region's country |
| `lookup_pc_specs` | none | PC maker and model from the firmware | the PC maker's site |

- **Inputs.** Names come from the PC's firmware and drivers. They are reduced to letters, digits
  and simple punctuation before a query. No program name, user name, path, command line, file
  content, secret, serial number or machine identifier is ever part of a query, whatever the
  sharing mode.
- **Search settings.** The client ([`tavily.rs`](../crates/tools/src/tavily.rs)):
  - asks for `search_depth: basic`, at most 5 results, and `include_domains` restricted to the
    sites above;
  - never asks for `include_answer`, raw page content, images or the extraction of a model-chosen
    URL;
  - keeps only `https` results from the allowed domains;
  - bounds titles and snippets.
- **Budget.** A turn makes at most 3 searches. A search costs 1 Tavily credit (1,000 free credits a
  month, then $0.008 a credit, as read on 3 October 2026).
- **Not an inspection.** A lookup never counts as an inspection of the PC, and it is not citable
  as `[O…]`.

## Citations and verification

- **Citation.** Each result is a web source cited `[W1]`, `[W2]`… (numbering runs across the
  turn). The answer cites a web fact right after it, as it does for measurements.
- **Checks** ([`grounding.rs`](../crates/agent/src/answer/grounding.rs)):
  - a quantity cited with `[W…]` must appear in that source's snippet;
  - a price (€, $, £, EUR, USD, GBP) must appear in the snippet of a `[W…]` source cited in its
    passage, to the written precision. A price cited only with `[O…]` is flagged: it is never a
    measurement of this PC;
  - a version number (two dots or more, or one dot after the word version, pilote or driver)
    cited with `[W…]` must appear in the snippet exactly;
  - a PC measurement cited with `[W…]` is not supported by the web, and a web number cited with
    `[O…]` is not supported by the PC.
- **Prompt.** When the tools are declared, the model is told that web snippets are not
  measurements and may be outdated, and that a known-issue match is a hint, not proof (confidence
  at most medium). A price is a dated range from its source, and a newer driver is suggested only
  if its notes fix the measured symptom or a security issue.
- **Follow-up turns.** Web sources stay with the turn that searched. A follow-up performs a new
  lookup, which may reuse a fresh local cache entry with new citation identifiers.

## Consent, key and display

- **Consent.** Web search is a separate, remembered consent (`privacy_set` with `web_search`). It
  is never active in local mode.
- **Key.** The Tavily key follows the Nebius key's path: `TAVILY_API_KEY` in the environment or in
  `%APPDATA%\pcagent\.env`. It is saved by `save_search_key`, never returned to the page and never
  logged. The page only learns whether a key is saved and its last four characters.
- **Steps.** The exact query is shown in the turn's steps (`tool_finished.query`).
- **Source cards.** They show the title, domain, publication date (or unknown), the time it was
  read, the snippet and the query. *Open in browser* calls `open_web_source`, which reads the URL
  from the stored turn, never from the page, and opens only an `https` address.

## Persistent cache and official pages

The GUI opens `web-cache.sqlite` beside its credential file, not inside the history database.
Two short-lived connections share its versioned cache table: one for query results and one for
official page text. They never hold a transaction across network requests. Cache failures fall
back to a live lookup; they never prevent diagnosis. Deleting this disposable database clears the
cache without changing conversations, settings or credentials.

Keys include the tool, query (case/whitespace normalized), sorted/deduplicated domains and country.
Prices and driver notes last 24 hours; known issues and manufacturer specifications last 7 days.
Official Microsoft page text lasts 24 hours. A cached result derived from page text cannot extend
that text's original expiry. Expiry and clock rollback produce a miss, not stale evidence. Only
successful typed responses are stored; network errors, refusals, keys and malformed responses
are not. Empty successful searches retain their meaning: no match in these sources, not proof
that the PC is unaffected.

`lookup_known_issue` first downloads a fixed HTTPS allowlist of Windows release-health pages.
It sends no KB/code in a URL, follows no redirects, requires HTML, and extracts article text
locally with the pinned HTML parser. Exact case-insensitive identifier boundaries prevent a
short KB number from matching a different longer number. Page errors are not mistaken for a
complete negative search. If pages fail or have no match, the existing Microsoft-restricted
Tavily search remains available. This entire path remains behind the existing web consent and
key-enabled tools. No model-supplied URL is fetched.

Source cards retain original read time and display “cached result from …” / “résultat en cache
du …”; observations also preserve reuse time. Cache hits get fresh `[W…]` identifiers, consume
no network-search slot and charge zero Tavily credits. Replay reads only the recorded tool output,
including its original usage and cache metadata; it opens neither this cache nor the network.

## Usage estimates

The core records web lookups, cache hits, known credits and requests with unknown usage separately
from model requests. The CLI and GUI show the web estimate separately; a combined estimate is
available only when both model and web usage are complete. A failure or missing Tavily usage is
unknown, never free. The $0.008/credit list-price estimate excludes free allocations, discounts
and account billing: it is not an invoice. A source found directly in official pages needs no
Tavily request and reports zero credits.

The list of shops per country remains a first cut, to be checked against real results.
