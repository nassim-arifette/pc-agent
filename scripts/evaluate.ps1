<#
.SYNOPSIS
Replay frozen scenarios, keeping every result and a CSV summary outside the repository.
.DESCRIPTION
Offline by default. -AllowCloud explicitly enables paid Nebius requests; only model IDs, never
keys, are passed as arguments. Each scenario/model pair starts a fresh process and replay cursor.
.EXAMPLE
.\scripts\evaluate.ps1 -OutputDirectory D:\pcagent-tmp\eval-offline
.EXAMPLE
.\scripts\evaluate.ps1 -Scenarios D:\pcagent-tmp\captures -OutputDirectory D:\pcagent-tmp\eval-live -AllowCloud -Models nvidia/nemotron-3-super-120b-a12b
#>
param(
    [string]$Scenarios = (Join-Path $PSScriptRoot '..\eval\scenarios'),
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Evaluator = 'D:\pcagent-target\release\pcagent-eval.exe',
    [string[]]$Models = @(),
    [switch]$AllowCloud,
    [switch]$Anonymize,
    [ValidateRange(1, 10)][int]$Repeat = 1
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
. (Join-Path $PSScriptRoot 'eval-files.ps1')
if (-not (Test-Path -LiteralPath $Evaluator -PathType Leaf)) { throw 'Evaluator not found; build pcagent-eval first.' }
if ($Models.Count -gt 0 -and -not $AllowCloud) { throw '-Models requires -AllowCloud.' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Choose a new output directory; campaign files are never overwritten.' }
$fullOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
$parent = Split-Path -Parent $fullOutput
if (-not (Test-Path -LiteralPath $parent -PathType Container)) { throw 'The output parent directory must exist.' }
$resolvedParent = (Resolve-Path -LiteralPath $parent).Path
$ancestor = [System.IO.DirectoryInfo]::new($resolvedParent)
while ($null -ne $ancestor) {
    if (Test-Path -LiteralPath (Join-Path $ancestor.FullName '.git')) { throw 'Campaign data must stay outside Git repositories.' }
    $ancestor = $ancestor.Parent
}
$inputs = @(Get-ChildItem -LiteralPath $Scenarios -Filter '*.json' -File | Sort-Object Name)
if ($inputs.Count -eq 0) { throw 'No scenario JSON files found.' }
$scenarioRecords = @(foreach ($inputFile in $inputs) {
    $scenario = Read-EvalJson $inputFile.FullName
    if ($scenario.schema_version -ne 1 -or $null -eq $scenario.initial -or $null -eq $scenario.calls) {
        throw 'Expected recorded scenarios; result files are not scenario inputs.'
    }
    [pscustomobject]@{
        file = $inputFile.FullName; file_sha256 = (Get-FileHash -LiteralPath $inputFile.FullName).Hash.ToLowerInvariant()
        origin = $scenario.initial.origin; captured_at_unix_ms = $scenario.captured_at_unix_ms
    }
})
New-Item -ItemType Directory -Path $fullOutput | Out-Null
$source = Save-EvalSource $fullOutput (Join-Path $PSScriptRoot '..')
Write-EvalJson (Join-Path $fullOutput 'manifest.json') ([ordered]@{
    schema_version = 1; created_at_unix_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    evaluator_sha256 = (Get-FileHash -LiteralPath $Evaluator).Hash.ToLowerInvariant()
    source = $source; scenarios = $scenarioRecords; models_requested = $Models
    allow_cloud = [bool]$AllowCloud; anonymize = [bool]$Anonymize; repeat = $Repeat
    grading = 'ungraded'; quality_claim = $null
})
$modelIds = if ($Models.Count -gt 0) { $Models } else { @('') }
$rows = @()
$grading = @()
$runNumber = 0
foreach ($inputFile in $inputs) {
    $origin = ($scenarioRecords | Where-Object { $_.file -eq $inputFile.FullName }).origin
    foreach ($modelId in $modelIds) {
        for ($iteration = 1; $iteration -le $Repeat; $iteration++) {
            $runNumber++
            $runId = 'run-{0:D4}' -f $runNumber
            $resultPath = Join-Path $fullOutput "$runId.json"
            $arguments = @('replay', $inputFile.FullName, '--out', $resultPath)
            if ($AllowCloud) { $arguments += '--allow-cloud' }
            if ($modelId) { $arguments += @('--model', $modelId) }
            if ($Anonymize) { $arguments += '--anonymize' }
            & $Evaluator @arguments 2> (Join-Path $fullOutput "$runId.stderr.log")
            $code = $LASTEXITCODE
            $result = if (Test-Path -LiteralPath $resultPath) { Read-EvalJson $resultPath } else { $null }
            $report = $result.run.report
            $rows += [pscustomobject]@{
                run = $runId; scenario = $inputFile.BaseName; scenario_origin = $origin
                scenario_sha256 = $result.scenario_sha256; tool_registry_sha256 = $result.tool_registry_sha256
                question_sha256 = if ($null -ne $result.question) { Get-EvalTextHash $result.question } else { $null }
                sharing = $result.sharing
                mode = $result.mode; model = $result.run.model; repeat = $iteration; exit_code = $code
                replay_complete = $result.replay_complete
                misses = if ($null -ne $result) { @($result.replay_misses).Count } else { $null }
                reused_calls = $result.reused_calls; duration_ms = $result.duration_ms
                execution_status = $report.execution_status; requests = $report.stats.inference_calls
                tool_calls = $report.stats.tool_calls; prompt_tokens = $report.stats.prompt_tokens
                completion_tokens = $report.stats.completion_tokens; cost_usd = $report.stats.cost.usd
                unverified = if ($null -ne $report.answer) { @($report.answer.unverified).Count } else { $null }
            }
            $grading += [pscustomobject]@{
                run = $runId; cause = ''; actions = ''; safety = ''; fidelity = ''; clarity = ''
                grader = ''; notes = ''
            }
            Write-Host "$runId : $($inputFile.BaseName), exit $code"
        }
    }
}
$rows | Export-Csv -LiteralPath (Join-Path $fullOutput 'summary.csv') -NoTypeInformation -Encoding UTF8
$grading | Export-Csv -LiteralPath (Join-Path $fullOutput 'scores.csv') -NoTypeInformation -Encoding UTF8
Write-Host "Saved $runNumber runs and provenance; scores.csv is ungraded. Prepare blind packets with scripts/grade.ps1. Offline runs do not measure model quality."
if (@($rows | Where-Object { $_.exit_code -ne 0 }).Count -gt 0) { exit 2 }
