# NVIDIA account model catalog

`Nebius::account_catalog` reads the saved account's
[`GET /v1/models?verbose=true`](https://api.tokenfactory.nebius.com/docs).
`pcagent models --allow-cloud --json` and the Rust GUI command `cloud_models` return only catalog
metadata. Neither sends measurements or exposes credentials to the page. The saved model need not
already exist for a first-run catalog request. A request has a bounded timeout and response size.

Only explicit NVIDIA Nemotron IDs are eligible. Unknown context, output limit, price or capability
stays `null`; an ID or model size does not establish any of those properties. Duplicate eligible IDs
are rejected rather than selecting an arbitrary price. Explicit capability flags take precedence;
the account's `supported_features` entries `tools` and `reasoning` provide positive evidence, while
absence of a feature is not a negative capability claim. A configured ID must exist in the account
catalog. An explicit refusal of tools is incompatible with PC Agent's tool loop.

Prices retain the account's decimal strings, source and retrieval time. Existing fixed-point cost
accounting uses these prices. No model default is changed by this lot: choosing a default or Super
to Ultra escalation still requires graded evaluation evidence. Model selection remains an explicit
saved backend setting.

The real account read on 3 October 2026 returned Nano, Super, Ultra and Lightning, all with
`tools` and `reasoning`. Super and Nano advertised 262,144 context tokens; Ultra and Lightning advertised
1,048,576. The API did not publish a maximum output-token limit. These are a dated account
observation, not values embedded in the implementation. The provider's
[Nemotron cookbook](https://github.com/nebius/token-factory-cookbook/blob/main/models/nemotron/README.md)
also documents the current family; account availability and prices remain authoritative.

Frontend wiring remains: refresh and display this catalog, show unavailable metadata clearly, and
save the chosen ID through the existing configuration path. Credentials continue to stay in Rust.
