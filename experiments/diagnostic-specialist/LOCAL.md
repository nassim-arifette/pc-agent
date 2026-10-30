# Run the published specialist locally

The release contains **LoRA adapter weights**, not the full NVIDIA model. This guide loads
`nvidia/Llama-3.1-Nemotron-Nano-4B-v1.1` at revision
`d552708a9d575fa8d4a690b988fd870d65279f98`, then attaches the published safetensors adapter.
No Nebius GPU VM is created. No training is performed.

There are two distinct demonstrations:

| Path | Local work | Network requirement |
| --- | --- | --- |
| Desktop investigation | Nano 4B selects real existing read-only PC tools; the normal controller executes them | The final configured analyst still uses Nebius Token Factory, with the existing sharing preview and consent |
| Frozen synthetic comparison | Base and adapter run on exactly the same saved test states, in one loaded process | None after downloading the dependencies, adapter and pinned base model |

Selecting the app's existing **local analysis** mode does not run this planner or call Nebius.
Do not confuse that rule-based mode with the local-model specialist. The specialist is not
added to the Token Factory model menu and cannot produce `submit_answer`.

## Requirements and licenses

- Windows with Python 3.10+ on `PATH` (tested here with Python 3.13 and CPU PyTorch).
- CPU: float32 inference needs roughly 16 GB for base weights alone; allow at least 24 GB
  available RAM and roughly 20 GB free disk for dependencies, downloads and caches. More is
  preferable when running the app simultaneously. CPU inference can take minutes per decision.
- Optional NVIDIA CUDA GPU: the script installs PyTorch 2.7.1 CUDA 12.6 and NF4 dependencies.
  Use a compatible NVIDIA driver. NF4 is CUDA-only; GPU memory needs depend on context and
  backend. CUDA execution is not validated on this CPU-only development VM.
- Initial internet access to PyPI/PyTorch, Hugging Face and the GitHub release. Review the
  [upstream model card and license](https://huggingface.co/nvidia/Llama-3.1-Nemotron-Nano-4B-v1.1)
  and the release's NVIDIA/Meta notices before downloading. The base weights are not
  redistributed by PC Agent.
- If access is gated in your environment, authenticate with Hugging Face's own tools locally;
  do not put a token in source, command history, screenshots or the specialist HTTP request.

## Windows startup

From the repository root in PowerShell:

```powershell
.\scripts\start-specialist.ps1
```

The default is CPU, with a dedicated virtual environment and release cache under
`$env:LOCALAPPDATA\pcagent\specialist`. First startup downloads the model; subsequent startups
reuse the Hugging Face cache. Keep the console open. Stop with Ctrl+C.

For a CUDA-capable NVIDIA machine:

```powershell
.\scripts\start-specialist.ps1 -Device cuda
```

If the repository is private, GitHub returns 404 for unauthenticated release downloads.
Download `Llama-PC-Agent-Nano4B-Specialist-v0.1.0.zip` from the
[release page](https://github.com/nassim-arifette/pc-agent/releases/tag/nano4b-specialist-v0.1.0)
using your authenticated browser, then run:

```powershell
.\scripts\start-specialist.ps1 -Archive "$HOME\Downloads\Llama-PC-Agent-Nano4B-Specialist-v0.1.0.zip"
```

The script verifies the frozen archive SHA-256, safe extraction paths and every extracted
manifest entry. It never requests a GitHub API token. For a jury without repository access,
the owner must publish the repository/release or separately provide the verified archive.

After the first successful model load, restart without downloads:

```powershell
.\scripts\start-specialist.ps1 -Offline
```

`-Offline` requires the already installed environment, extracted adapter and cached pinned
base; it sets Hugging Face offline flags. It does not make the desktop's final analyst local.
`-SetupOnly` installs the packages and adapter without loading/downloading the full base.
Use `-Cache PATH` to place the virtual environment and release on a different disk.

### Inspect provenance

```powershell
Invoke-RestMethod http://127.0.0.1:8737/health | Format-List
```

Health is available only after model loading. It reports protocol version, base ID and pinned
revision, device/precision, synthetic-only training and the **adapter weight file** SHA-256:

```text
b85a03348ebeb27a327bcf53c5b3da3bde931a34337650d0eccb9d5887dbd459
```

The desktop verifies that exact identity again for each decision. Health performs no generation
and returns no question, process data, file path or credentials.

## Desktop demonstration: real measurements, then the analyst

1. Start the local server and wait for `Specialist ready`.
2. Open **Settings → Local Nano 4B specialist → Check local model**. Confirm the base revision,
   adapter hash and CPU/CUDA device, then explicitly enable **Use the local specialist first**.
3. Start a new text investigation such as “My PC feels slow”. Keep the normal sharing preview:
   enabling the local specialist does **not** opt out of the cloud analyst or change consent.
4. The conversation shows the local decision, the actual tool execution and the handoff.
   The final analyst receives the real observations with their existing citations, not a
   synthetic recording or an asserted diagnosis from Nano 4B.
5. Inspect the answer's evidence and per-model usage. Local generation is identified separately
   from Token Factory and has no provider charge; electricity/device costs are not estimated.

The integration is off by default and currently applies to ordinary text investigations only.
Image turns and selected-evidence Explain/Investigate retain their current path. An invalid
JSON, foreign observation ID, repeated call, unsupported tool/argument, unavailable server,
identity mismatch or overlong context causes an explicit handoff. Nothing is silently repaired.
An `ask` decision is passed to the analyst as a clarification request; no user answer or symptom
timing is invented. The analyst remains responsible for the final checked reply.

The planner sees a reduced allowlisted projection of the already privacy-filtered model-facing
measurements: supported numeric metrics and units, status, limitations and retained history
samples. Omitted history points are counted, not replaced with invented values. Names and
paths are not added to the projection. The full real evidence remains available to the analyst
and verifier. Live PC projections differ from the simplified synthetic training distribution:
the published synthetic scores are **not** accuracy claims about this integration.

Stop cancels the app's next steps and discards a pending local response. An already-running
PyTorch generation may finish server-side (without executing any tools); the server rejects
another inference as busy until it ends. Restart the console to abort computation immediately.
The app's existing request/time budgets still apply, including on slow CPUs.

## Fully local paired comparison

With the server running:

```powershell
$cache = "$env:LOCALAPPDATA\pcagent\specialist"
& "$cache\venv\Scripts\python.exe" experiments/diagnostic-specialist/compare_local.py `
  --data "$cache\Llama-PC-Agent-Nano4B-Specialist-v0.1.0\data\test.jsonl" `
  --out "$HOME\specialist-comparison.json" --limit 4
```

Use `--limit 0` for the full frozen set. Each row runs with the adapter disabled and enabled,
using identical tokenizer, prompt, precision, greedy decoding and token limits. JSON output
includes raw outputs, strict scores, token counts, latency and identity. Invalid base **and**
adapter outputs remain failures. This does not replay a live PC and does not require a
Nebius key. The original sequential replay and its limitations remain in `rollout.py`.

CPU float32 and CUDA NF4 are different numerical configurations: neither the historical
L40S latencies nor its exact score should be claimed as reproduced by a CPU smoke test.
All cases are synthetic, not expert-validated diagnoses. No latency improvement is promised.

## Other hosts / manual installation

The Python service is portable, although native app tools target Windows. Create a virtual
environment, install the appropriate official PyTorch 2.7.1 build, then install
`requirements-local.txt`. For CUDA NF4 also install `bitsandbytes==0.45.5`. Verify/extract
the release with `download_adapter.py --directory CACHE [--archive ZIP]`, then run:

```text
python local.py --adapter CACHE/Llama-PC-Agent-Nano4B-Specialist-v0.1.0/adapter --device cpu
```

All paths above are relative to `experiments/diagnostic-specialist` for manual invocation.
Use `--offline` after caching the base, or `--device cuda --nf4` for the training-like precision.

## Service boundaries and checks

The service binds **only** to `127.0.0.1:8737`. There is no configurable remote endpoint, proxy,
redirect, CORS permission, tool execution, file-reading API or provider credential. It rejects
browser Origin / Fetch-Site headers and foreign Host headers to resist drive-by browser calls
and DNS rebinding. It is not a security boundary against other programs running as the same
local user. Requests and model outputs are not logged by the server.

```powershell
python -m unittest discover -s experiments/diagnostic-specialist -p 'test_*.py'
cargo test -p pcagent-agent -p pcagent-nebius --lib --locked
```

Protocol/controller unit tests use clearly identified fake planners, not fake model benchmark
results. They require no weights, GPU or paid provider.
