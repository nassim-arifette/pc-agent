# Model comparison

> **Archived on 2026-09-28.** These results measure the earlier agent contract, in which the model only selected finding IDs. They do not predict answer quality once the model writes its answers. Nemotron 3 Ultra was not in the catalog when this ran. The new evaluation is work package 7 of [the October plan](../plan.md).

Three NVIDIA Nemotron models from the Nebius Token Factory catalog, compared on the same eight
typical questions: the default `nvidia/nemotron-3-super-120b-a12b` and two models about five times
cheaper per token, `nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B` and `nvidia/Nemotron-3_5-Lightning`.

## Method

- **When and where.** 2026-09-27, on the development laptop (Windows 11, 11.7 GiB RAM, GeForce
  MX350), with compact model payloads and NVML readings.
- **Runs.** `scripts/compare-models.ps1` runs `pcagent diagnose "<question>" --allow-cloud --json`
  for each question in `scripts/questions.txt` (four in French, two in English, two about
  hardware). For each question, the three models run one after the other, so they observe the
  PC within about a minute of each other. There was one round per question and model.
- **Same settings for every model.** Temperature 0; at most 4,096 output tokens per request,
  reasoning included; `tool_choice: "required"` until a real inspection; at most six requests,
  twelve tools and 90 s per diagnosis; at most 30 s per request.
- **Prices.** Prices come from the Nebius catalog (`/v1/models?verbose=true`), read that day, per
  million tokens (input / output): Super $0.30 / $0.90, Nano and Lightning $0.06 / $0.24. Costs are
  the tokens the API reported, priced at these rates.

## Results

| Model | Completed | First request | Input tokens per diagnosis | Output tokens per diagnosis | Cost per completed diagnosis | Duration |
| --- | --- | --- | --- | --- | --- | --- |
| Super | 8 / 8 | 4,542–4,828 tokens | mean 13,596 (9,656–19,776) | mean 354 (264–496) | mean $0.0044 ($0.0032–$0.0064) | mean 7.8 s (6.1–11.2) |
| Nano | 6 / 8 | 4,468–4,859 tokens | mean 10,716 (9,234–11,677) | mean 2,687 (1,823–3,557) | mean $0.0013 ($0.0011–$0.0015) | mean 31.3 s (23.3–37.8) |
| Lightning | 0 / 8 | 4,534–4,546 tokens | 4,541 (first request only) | 4,096 (limit reached) | none; each failure billed about $0.0013 | mean 15.8 s |

Selected findings (identical PC state is not guaranteed between models):

| Question | Super | Nano |
| --- | --- | --- |
| Pourquoi mon PC rame ? | low available memory | hard-fault pressure |
| Est-ce que j'ai besoin de plus de RAM ? | low available memory | insufficient evidence |
| Why is my PC slow? | low available memory | failed (second request, 53 s) |
| Mon disque est-il le problème ? | insufficient evidence | insufficient evidence |
| Pourquoi mon PC chauffe et le ventilateur tourne fort ? | insufficient evidence | failed (second request, 62 s) |
| Quel programme utilise le plus de mémoire ? | largest memory contributor | largest memory contributor |
| Mes jeux saccadent, est-ce la carte graphique ? | largest memory contributor | largest memory contributor |
| Is my CPU the bottleneck? | insufficient evidence | insufficient evidence |

## Findings

- **Super** completed every diagnosis with two or three requests and one or two tools. It is the
  only one of the three that reports its reasoning tokens (233–417 per diagnosis).
- **Nano** is 3.4 times cheaper per completed diagnosis, not 5 times: it writes 7.6 times more
  output tokens (reasoning included; the API does not report them separately for this model). It
  is 4 times slower. Two of eight diagnoses failed while waiting for the second reply, after
  53 s and 62 s in total; both are consistent with the 30-second request limit. Those runs
  were reported as `network`; pcagent now reports a request that runs out of time as `timeout`.
  On the six completed questions, it selected the same finding as Super four times; the two
  differences stay on memory. The PC changed between runs, so this is not a quality score.
- **Lightning** is unusable with these settings. In 16 of 16 runs (8 in this comparison, 8 rerun
  the same day), the first reply stopped at the 4,096-token output limit (`finish_reason:
  "length"`) before any tool call, after about 15 s. Such a reply is billed (about 4,540 input and
  4,096 output tokens, about $0.0013, the price of a completed Nano diagnosis). pcagent now ends
  the diagnosis as `output_limit` and counts those tokens in the usage and cost. A larger budget or
  a way to limit reasoning was not tested: at about 270 tokens/s, 8,192 tokens would already reach
  the 30-second request limit.

**Decision:** keep Super as the default. Nano is an option for users who accept diagnoses of about
30 s and occasional timeouts; a longer per-request limit would be needed to make it reliable.
Lightning needs a reasoning control before another trial.

## Check after merging citations and budget extensions

Later the same day, after merging observation citations, coverage states and consent-based budget
extensions from `main`, Super was rerun with the merged build: 54 of 57 diagnoses completed (the
eight questions twice, plus repeats of "Is my CPU the bottleneck?"). The three failures were
answers rejected by local validation: two conclusions citing something other than a current
finding or a citable observation (`invalid_finding`) and one tool call with arguments outside its
schema (`invalid_tool`). With a local debug trace, 25 further runs of that question did not
reproduce them. On the eight questions, a diagnosis cost $0.0038 to $0.0060 (mean $0.0044). The
first request grew to 4,620 to 5,004 tokens with the citation instructions.

## Limits

One PC, eight questions, one round per question and model. The live PC state differs between
runs, so the selected findings are not ground truth, and a failure rate over eight runs is only an
indication. Catalog prices and per-model limits can change; pcagent reads them at run time and
dates them in each report.

## Reproduce

```powershell
cargo build --release -p pcagent
.\scripts\compare-models.ps1 -Output comparison.csv
.\scripts\compare-models.ps1 -Models nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B -Repeat 3 -Output nano.csv
```

The default 24 runs cost about $0.05 at the prices above. Every run is a live, paid request that
sends each question verbatim plus minimized observations (see the README's cloud disclosure).
