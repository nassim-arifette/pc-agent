<#
.SYNOPSIS
Exercise campaign/import/blinding/grading offline, using only synthetic evidence and test answers.
.DESCRIPTION
Requires a built evaluator; never builds, collects a real PC or calls a provider. Keeps all test
artifacts in a new local directory. Synthetic answers and scores are harness tests, not benchmarks.
#>
param(
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Evaluator = 'D:\pcagent-target\release\pcagent-eval.exe'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'eval-files.ps1')
$output = New-EvalDirectory $OutputDirectory
$scenarios = Join-Path $output 'scenarios'
$references = Join-Path $output 'references'
[IO.Directory]::CreateDirectory($scenarios) | Out-Null
[IO.Directory]::CreateDirectory($references) | Out-Null
[IO.File]::Copy((Join-Path $PSScriptRoot '..\eval\scenarios\memory-pressure.json'), (Join-Path $scenarios 'synthetic.json'), $false)
$campaign = Join-Path $output 'campaign'
& (Join-Path $PSScriptRoot 'evaluate.ps1') -Scenarios $scenarios -OutputDirectory $campaign -Evaluator $Evaluator
if ($LASTEXITCODE -ne 0) { throw 'Offline synthetic campaign failed.' }
$baseline = Read-EvalJson (Join-Path $campaign 'run-0001.json')
if ($baseline.scenario_origin -ne 'synthetic_fixture') { throw 'Fixture origin was lost.' }
# These explicit test answers exercise imported result shapes. They are never cloud runs.
$testRun = Read-EvalJson (Join-Path $campaign 'run-0001.json')
$testRun.mode = 'nebius_replay'
$testRun | Add-Member -NotePropertyName run -NotePropertyValue ([pscustomobject]@{
    model = 'synthetic-test-model-b'; duration_ms = 7
    report = [pscustomobject]@{ answer = [pscustomobject]@{ text = 'Synthetic B answer for harness validation only.'; unverified = @() }; stats = [pscustomobject]@{ cost = [pscustomobject]@{ usd = '0.001' } } }
})
Write-EvalJson (Join-Path $campaign 'run-0002.json') $testRun
$traceFile = Join-Path $references 'tools.json'
$frames = @(
    @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2025-11-25'; capabilities = @{}; clientInfo = @{ name = 'synthetic-test-client'; version = '1' } } },
    @{ jsonrpc = '2.0'; method = 'notifications/initialized' },
    @{ jsonrpc = '2.0'; id = 2; method = 'tools/call'; params = @{ name = 'get_initial_context'; arguments = @{} } },
    @{ jsonrpc = '2.0'; id = 3; method = 'tools/call'; params = @{ name = 'get_system_snapshot'; arguments = @{} } }
) | ForEach-Object { ConvertTo-Json -InputObject $_ -Depth 10 -Compress }
$replies = @($frames | & $Evaluator mcp --scenario (Join-Path $scenarios 'synthetic.json') --allow-cloud --allow-recording --out $traceFile 2> (Join-Path $output 'mcp.stderr.log'))
if ($LASTEXITCODE -ne 0 -or $replies.Count -ne 3) { throw 'Offline MCP recording failed.' }
foreach ($reply in $replies) { if (($reply | ConvertFrom-Json).jsonrpc -ne '2.0') { throw 'MCP stdout was contaminated.' } }
$trace = Read-EvalJson $traceFile
Write-EvalJson (Join-Path $references 'reference.json') ([ordered]@{
    schema_version = 1; configuration = 'C'; model = 'synthetic-test-model-c'; reasoning_effort = 'synthetic'; harness_test = $true
    scenario_sha256 = $trace.scenario_sha256; tool_registry_sha256 = $trace.tool_registry_sha256
    question = $trace.question; sharing = $trace.sharing; answer = 'Synthetic C answer for harness validation only.'
    evidence_basis = 'same_replay'; trace_file = 'tools.json'; duration_ms = $null; cost_usd = $null
    usage = $null; unverified = $null
})
$groundTruth = Join-Path $output 'ground-truth.json'
Write-EvalJson $groundTruth @([pscustomobject]@{
    scenario_sha256 = $baseline.scenario_sha256
    ground_truth = 'Synthetic harness fixture only; this is not a real diagnostic judgment.'
    limitations = @('No human or model quality claim is made by this test.')
})
$packets = Join-Path $output 'packets'
& (Join-Path $PSScriptRoot 'grade.ps1') -CampaignDirectory $campaign -ReferenceDirectory $references -GroundTruthPath $groundTruth -OutputDirectory $packets -Evaluator $Evaluator -Seed 20261003
$map = Read-EvalJson (Join-Path $packets 'private\mapping.json')
if (@($map.answers).Count -ne 2 -or 'D' -notin $map.references_absent -or 'E' -notin $map.references_absent) { throw 'Missing references were not preserved.' }
if ($null -ne ($map.answers | Where-Object configuration -eq 'C').cost_usd) { throw 'Unknown reference cost became zero.' }
foreach ($packet in @(Get-ChildItem -LiteralPath (Join-Path $packets 'grader') -Filter '*.json' -File)) {
    $text = Get-Content -LiteralPath $packet.FullName -Raw -Encoding UTF8
    if ($text -match 'synthetic-test-model|cost_usd|duration_ms|configuration') { throw 'A grading packet revealed its model or run metrics.' }
}
$blankSummary = Join-Path $output 'ungraded.json'
& (Join-Path $PSScriptRoot 'grade-summary.ps1') -PacketDirectory $packets -Output $blankSummary
$blank = Read-EvalJson $blankSummary
if (@($blank.graded_answers).Count -ne 0 -or @($blank.ungraded_labels).Count -ne 2) { throw 'Blank scores were treated as judgments.' }
$rows = @(Import-Csv -LiteralPath (Join-Path $packets 'grader\scores.csv'))
foreach ($row in $rows) {
    $row.grader = 'synthetic-test-grader'
    foreach ($criterion in @('cause', 'actions', 'safety', 'fidelity', 'clarity')) { $row.$criterion = '2' }
    $row.unsafe_recommendation = 'true'; $row.unmarked_invented_number = 'false'; $row.missing_capture = 'false'
    $row.notes = 'Synthetic test of critical-failure retention; no real answer was graded.'
}
$filledScores = Join-Path $output 'test-scores.csv'
$rows | Export-Csv -LiteralPath $filledScores -NoTypeInformation -Encoding UTF8
$gradedSummary = Join-Path $output 'graded.json'
& (Join-Path $PSScriptRoot 'grade-summary.ps1') -PacketDirectory $packets -Scores $filledScores -Output $gradedSummary
$graded = Read-EvalJson $gradedSummary
if (@($graded.comparisons).Count -ne 0 -or @($graded.graded_answers | Where-Object unsafe_recommendation -eq $true).Count -ne 2) {
    throw 'Synthetic scores established a real ratio or lost a critical failure.'
}
Write-Host 'PASS: provenance, exact MCP evidence, blind identities, missing references, unknown cost, blank scores and synthetic/critical-failure gates. No model comparison was performed.'
