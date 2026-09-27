<#
.SYNOPSIS
Runs `pcagent diagnose --allow-cloud --json` for every question and model and writes one CSV row
per run: success, requests, tokens, estimated cost, duration and the selected finding.

.DESCRIPTION
Each run is a live, paid Nebius request (a few US cents at most). The PC's state changes between
runs, so a model's selected finding can differ for reasons other than the model. Models are run
one after the other for each question to keep their observations close in time.
The key comes from the environment or %APPDATA%\pcagent\.env; NEBIUS_MODEL_ID is set per run.

.EXAMPLE
.\scripts\compare-models.ps1 -Output model-comparison.csv
.\scripts\compare-models.ps1 -Models nvidia/nemotron-3-super-120b-a12b -Repeat 2
#>
param(
    [string[]]$Models = @(
        'nvidia/nemotron-3-super-120b-a12b',
        'nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B',
        'nvidia/Nemotron-3_5-Lightning'
    ),
    [string]$Questions = (Join-Path $PSScriptRoot 'questions.txt'),
    [string]$Pcagent = (Join-Path $PSScriptRoot '..\target\release\pcagent.exe'),
    [string]$Output = 'model-comparison.csv',
    [ValidateRange(1, 10)][int]$Repeat = 1
)
$ErrorActionPreference = 'Stop'
# pcagent writes UTF-8; decode it as such so French text survives the JSON parse.
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
if (-not (Test-Path $Pcagent)) { throw "pcagent not found at $Pcagent; build it with cargo build --release -p pcagent" }
$list = Get-Content -Encoding UTF8 $Questions | Where-Object { $_.Trim() -and -not $_.TrimStart().StartsWith('#') }
$previousModel = $env:NEBIUS_MODEL_ID
$rows = @()
try {
    foreach ($question in $list) {
        foreach ($round in 1..$Repeat) {
            foreach ($model in $Models) {
                $env:NEBIUS_MODEL_ID = $model
                $watch = [Diagnostics.Stopwatch]::StartNew()
                $text = (& $Pcagent diagnose $question --allow-cloud --json 2>$null) -join "`n"
                $exit = $LASTEXITCODE
                $watch.Stop()
                $row = [ordered]@{
                    question = $question; model = $model; round = $round; exit_code = $exit
                    seconds = [math]::Round($watch.Elapsed.TotalSeconds, 1)
                }
                try {
                    $d = $text | ConvertFrom-Json
                    $s = $d.stats
                    $first = if ($s.requests.Count -gt 0) { $s.requests[0].prompt_tokens } else { $null }
                    $row += [ordered]@{
                        status = $d.status; error = $d.error
                        requests = $s.inference_calls; tool_calls = $s.tool_calls; inspections = $s.inspection_calls
                        first_prompt_tokens = $first; prompt_tokens = $s.prompt_tokens
                        completion_tokens = $s.completion_tokens; reasoning_tokens = $s.reasoning_tokens
                        cost_usd = $s.cost.usd; selected = $d.selected.classification
                        tools = ($d.tool_events | ForEach-Object { $_.tool }) -join '+'
                    }
                } catch {
                    $row.status = 'no_json'
                }
                $rows += [pscustomobject]$row
                Write-Host ("{0,-40} {1,-38} {2,-8} {3,6}s {4}" -f $model, $question, $row.status, $row.seconds, $row.cost_usd)
            }
        }
    }
} finally {
    $env:NEBIUS_MODEL_ID = $previousModel
}
$rows | Export-Csv -NoTypeInformation -Encoding UTF8 -Path $Output
Write-Host "Wrote $($rows.Count) runs to $Output"
