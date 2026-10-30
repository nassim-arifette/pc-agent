# PC Agent next-measure specialist

An **experimental, synthetic-only** adaptation of
[`nvidia/Llama-3.1-Nemotron-Nano-4B-v1.1`](https://huggingface.co/nvidia/Llama-3.1-Nemotron-Nano-4B-v1.1).
The small model proposes a next read-only measurement, asks for timing, summarizes evidence
for the general analyst, or hands off when coverage is insufficient. It is not a replacement
for the existing large Nemotron analyst. The optional desktop integration and the fully local
base/adapter comparison are documented in [the local jury guide](LOCAL.md).

## Scope and safety

- No real-PC captures, personal traces, teacher-generated diagnoses, or expert-validation claim.
- CPU, memory, disk, normal, and insufficient-coverage synthetic episodes.
- A closed subset of real PC Agent inspection calls, with exact allowed arguments.
- No command execution, administration, remediation, or modification authority.
- Resource correlation is not a proven root cause, hardware defect, or purchase justification.

The payloads are simplified synthetic projections inspired by PC Agent's observation contract,
not an assertion that every collector emits precisely this reduced representation. The replay
only serves stored results for exact calls and explicitly stops on unsupported calls.

## Data and separation

```bash
python prepare.py --out data
python -m unittest -v test_experiment
```

The fixed generator creates 420 training decisions from 105 episodes / 15 scenario parents,
40 validation decisions from 10 episodes / 5 parents, 40 regression decisions from 10 episodes /
10 previously inspected parents, and 60 final-test decisions from 15 episodes / 15 fresh parents.
Parents are allocated **before** numeric/symptom variants. Parent IDs, episode IDs,
rubrics, and future observations are not model inputs. The manifest records SHA-256 hashes and
programmatic provenance; automatic checks do not constitute independent expert review.

These are held-out **parents within shared diagnostic domains**, not held-out domains,
real machines, or evidence of real-world diagnostic accuracy. Many decisions share a common
contract and policy; the effective diversity is much smaller than the row count suggests.

### Serialization regression and final-test provenance

The first prototype used insertion-order JSON during generation but sorted JSON in stored
states. Reloading a state therefore changed its actual prompt. Strong teacher-forced scores
did not reproduce in the sequential demo. `prompt()` now uses canonical sorted-key JSON;
tests assert that every saved state reconstructs its exact training/evaluation prompt.

The same two-epoch recipe, previously selected on validation, is retrained from the base model
after that format fix. The previously inspected test parents are **regression-only**; fresh
parents 30–44 form the final test, frozen before retraining. No further candidate is selected
on the final test. Published prototype results are historical, not the final benchmark.

## Reproduce on one 48 GB L40S

Python 3.12, CUDA-capable Linux, and a compatible NVIDIA driver are required for the GPU steps.
On Ubuntu, install `python3-venv python3-dev build-essential` first; Triton needs `Python.h`.
Use an isolated environment; the optional dependency TRL is pinned for environment provenance,
but training uses Transformers Trainer and explicit completion-only labels rather than relying
on a version-dependent TRL collator.

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install torch==2.7.1 --index-url https://download.pytorch.org/whl/cu126
pip install -r requirements.txt
python prepare.py --out data
python audit.py --data data --out outputs/token-audit.json
python train.py --data data --out outputs/smoke --max-steps 2
python evaluate.py --data data/validation.jsonl --adapter outputs/smoke/adapter \
  --out outputs/smoke-reloaded.json --limit 4
python train.py --data data --out outputs/epoch2 --epochs 2
python evaluate.py --data data/validation.jsonl --adapter outputs/epoch2/adapter \
  --out outputs/epoch2-validation.json
```

Each run requires a fresh output path. Smoke output is a plumbing check, not the final weights.
Choose any further training **only using validation**, then freeze the candidate before the
paired held-out evaluation:

```bash
python evaluate.py --data data/test.jsonl --out outputs/base-test.json
python evaluate.py --data data/test.jsonl --adapter outputs/epoch2/adapter \
  --out outputs/adapter-test.json
python demo.py --data data/test.jsonl --adapter outputs/epoch2/adapter \
  --out outputs/demo.json
python report.py --data data/test.jsonl --base outputs/base-test.json \
  --adapted outputs/adapter-test.json --out outputs/RESULTS.md
python rollout.py --data data/test.jsonl --out outputs/rollouts-base.json
python rollout.py --data data/test.jsonl --adapter outputs/epoch2/adapter \
  --out outputs/rollouts-adapter.json
```

`evaluate.py` loads a fresh base and reloads the saved adapter from disk. The base revision is
pinned in `contract.py`; NF4 double quantization, BF16 compute, deterministic decoding, prompts,
seed, and generation limits are identical. No JSON repair, constrained decoding, or per-model
prompt changes are used. Raw generations, truncations, token counts and latencies are retained.

`encode_row` verifies the chat-template prefix, masks every prompt token with `-100`, and
rejects sequences over 2,048 tokens rather than silently truncating the expected response.
QLoRA uses rank 16, alpha 32, dropout 0.05, all linear layers, learning rate `1e-4`, microbatch
1, accumulation 16, BF16, and gradient checkpointing. Training records package versions,
loss curves, actual runtime, GPU model, and peak allocated memory.

## How to interpret the scores

JSON/contract validity measures formatting and allowed calls, not diagnostic correctness.
Decision and next-measure scores use the explicit synthetic policy, with listed alternative
acceptable measurements, independently of unrelated contract-field failures. Hypothesis support
and evidence-reference checks reject unsupported categories and fabricated IDs; they do not
prove that each cited observation independently supports the whole interpretation.
Missing-evidence tags are intentionally exact and reject synonymous free text; the strict score
is therefore a contract-following metric, not a clinical or hardware-diagnostic accuracy score.
The report also includes per-domain and whole-episode success, handoffs, repeats, truncations,
and warmup-excluded latency. Decisions within an episode are not independent samples.

The sequential demo is a separate exact-call replay. An accepted alternative next measurement
in the decision rubric may not exist in that replay; such a call stops as unsupported instead
of inventing a result. There is no comparison here to the large Token Factory analyst and no
claim that the specialist is faster or better than that analyst.

`rollout.py` checks every frozen test episode without forcing the next correct state. Its
strict completion count requires every visited state and decision to match a saved rubric.
No user clarification is synthesized; a state differing from the saved steps is unscored
(`null`) and is not counted as strict success. This conservative exact-fixture coverage is
separate from the teacher-forced per-decision and complete-episode scores.

## Weight distribution and licensing

Distribute **only the PEFT adapter**, tokenizer/configuration, dataset generator or synthetic
data, manifest, environment lock, measured results, and replay trace as release assets.
Do not commit model weights, raw captures, SSH keys, account credentials, cloud ledgers,
or a downloaded base model to Git. Use SHA-256 checksums for the release archive.

The repository's MIT license covers its code, not an automatic relicensing of NVIDIA or Meta
model materials. The base model card specifies the
[NVIDIA Open Model License](https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/)
and references the [Llama 3.1 Community License](https://www.llama.com/llama3_1/license/).
Recipients must obtain the pinned base separately and comply with applicable upstream terms.
**Built with Llama.** Include upstream license/attribution notices with the distributed artifact.

## Cloud cost and teardown

The initial Nebius quote for one `gpu-l40s-a / 1gpu-16vcpu-64gb` is $1.7468/hour,
excluding storage and other separately metered charges. The experiment targets a total below
$20, with a six-hour Compute-stop watchdog and guest shutdown timers as defense in depth.
Guest shutdown alone is **not** a billing guarantee. After retrieving and verifying artifacts,
stop/delete the Compute instance and delete any remaining temporary disk/public allocation.
Verify the project resources, not merely that SSH disconnected. No cloud account credentials
belong in release artifacts. A runtime estimate is not an invoice or verified credit balance.

## Published experimental run

Download the attached experiment ZIP from the
[experimental release](https://github.com/nassim-arifette/pc-agent/releases/tag/nano4b-specialist-v0.1.0)
for the adapter, tokenizer, generated splits, exact environment, raw results, replay traces,
upstream notices and SHA-256 manifest. GitHub's autogenerated source archives contain code
only, not the trained adapter. The scripts are proposed separately in
[PR #122](https://github.com/nassim-arifette/pc-agent/pull/122); publication does not merge it.

On the final 60 synthetic decisions, the adapter passes 52 strict checks versus zero for the
base model. Whole teacher-forced episodes pass in 8/15 cases versus zero; sequential exact-call
replays pass in 6/15 versus zero, with seven unsupported-call stops and two unscored episodes.
The measured median decision latency is 3.45 seconds for the adapter versus 2.50 for the base,
so this run demonstrates no speed gain. These are finite synthetic-policy scores, not real-PC
diagnostic accuracy. No Windows-app integration is included.

The run completed on an L40S. The instance was authoritatively stopped (`STOPPED`) and deleted
(`NotFound`); final project inventories contained no instances, disks or allocations. The ZIP
includes a sanitized teardown report and compute-only estimate, explicitly not an invoice.
