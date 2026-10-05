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
composed on the PC from local measurements ([`external_tools.rs`](../crates/agent/src/external_tools.rs)).

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
- **Search settings.** The client ([`tavily.rs`](../crates/agent/src/tavily.rs)):
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
- **Follow-up turns.** Web sources stay with the turn that searched. A follow-up searches again
  rather than citing old snippets.

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

## Not done yet

- The release health pages for compatibility holds, which C6 plans to download without a search,
  are not fetched.
- There is no local cache of results (plan: 24 h for prices and driver notes, 7 days for known
  issues).
- Tavily's cost is not added to the turn's displayed cost; the credits used are counted by
  `Workstation::web_credits`.
- The list of shops per country is a first cut, to be checked against real results.
