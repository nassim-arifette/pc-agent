<#
.SYNOPSIS
Prepare local blinded grading packets from a campaign and separately saved C/D/E references.
.DESCRIPTION
No provider or judge calls. Raw files and the private identity map stay outside Git. The grader
directory hides configuration, model, bill, latency and source filenames. Share only after C5 review.
#>
param(
    [Parameter(Mandatory)][string]$CampaignDirectory,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$ReferenceDirectory,
    [string]$GroundTruthPath,
    [string]$Evaluator = 'D:\pcagent-target\release\pcagent-eval.exe',
    [int]$Seed = (Get-Random -Maximum 2147483647)
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'eval-files.ps1')
$manifest = Read-EvalJson (Join-Path $CampaignDirectory 'manifest.json')
if ($manifest.schema_version -ne 1) { throw 'Unsupported campaign manifest.' }
if (-not (Test-Path -LiteralPath $Evaluator -PathType Leaf)) { throw 'Evaluator not found.' }
$output = New-EvalDirectory $OutputDirectory
$private = Join-Path $output 'private'
$grader = Join-Path $output 'grader'
[IO.Directory]::CreateDirectory($private) | Out-Null
[IO.Directory]::CreateDirectory($grader) | Out-Null
$catalog = @{}
$sequence = 0

function Get-Preview($Source, [string]$Sharing, $Calls) {
    $script:sequence++
    $destination = Join-Path $private ('preview-{0:D4}.json' -f $script:sequence)
    $arguments = @('replay', $Source.file, '--out', $destination)
    if ($Sharing -eq 'anonymized') { $arguments += '--anonymize' }
    if ($null -ne $Calls) {
        $callFile = Join-Path $private ('calls-{0:D4}.json' -f $script:sequence)
        $selected = @($Calls | ForEach-Object { [pscustomobject]@{ tool = $_.tool; arguments = $_.arguments } })
        if ($selected.Count -gt 0) {
            Write-EvalJson $callFile $selected
            $arguments += @('--calls', $callFile)
        }
    }
    & $Evaluator @arguments 2> "$destination.stderr.log"
    if ($LASTEXITCODE -notin @(0, 2) -or -not (Test-Path -LiteralPath $destination)) {
        throw 'Cannot reconstruct the minimized grading evidence; inspect the private preview log.'
    }
    Read-EvalJson $destination
}

foreach ($source in @($manifest.scenarios)) {
    if ((Get-FileHash -LiteralPath $source.file).Hash.ToLowerInvariant() -ne $source.file_sha256) {
        throw 'A campaign scenario changed after the run. Restore its original file before grading.'
    }
    $preview = Get-Preview $source '' $null
    if ($preview.scenario_sha256 -notmatch '^[0-9a-f]{64}$' -or $preview.tool_registry_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'Grading needs an evaluator with scenario and registry fingerprints.'
    }
    $catalog[$preview.scenario_sha256] = [pscustomobject]@{ source = $source; preview = $preview }
}
$truth = @{}
if ($GroundTruthPath) {
    foreach ($item in @(Read-EvalJson $GroundTruthPath)) {
        if (-not $catalog.ContainsKey([string]$item.scenario_sha256)) { throw 'Ground truth refers to an unknown scenario.' }
        $truth[$item.scenario_sha256] = $item
    }
}
$entries = @()
$excludedRuns = @()
foreach ($file in @(Get-ChildItem -LiteralPath $CampaignDirectory -Filter 'run-*.json' -File | Sort-Object Name)) {
    $result = Read-EvalJson $file.FullName
    if ($result.mode -ne 'nebius_replay') { continue }
    if ([string]::IsNullOrWhiteSpace($result.run.report.answer.text)) {
        $excludedRuns += [pscustomobject]@{ file = $file.FullName; model = $result.run.model; scenario_sha256 = $result.scenario_sha256; reason = 'No final answer was produced; this run cannot be silently discarded from a quality claim.' }
        continue
    }
    $entries += [pscustomobject]@{
        configuration = 'B'; model = $result.run.model; result = $result
        source_file = $file.FullName; answer = $result.run.report.answer.text
        unverified = $result.run.report.answer.unverified; calls = @($result.calls)
        evidence_basis = 'same_replay'; artifacts = @(); duration_ms = $result.run.duration_ms
        cost_usd = $result.run.report.stats.cost.usd; reference = $null
    }
}
if ($ReferenceDirectory) {
    foreach ($file in @(Get-ChildItem -LiteralPath $ReferenceDirectory -Filter '*.json' -File | Sort-Object Name)) {
        $reference = Read-EvalJson $file.FullName
        if (-not $reference.configuration) { continue } # The directory may also hold MCP traces.
        if ($reference.schema_version -ne 1 -or $reference.configuration -notin @('C', 'D', 'E') -or
            [string]::IsNullOrWhiteSpace($reference.answer) -or [string]::IsNullOrWhiteSpace($reference.model)) {
            throw 'Invalid C/D/E reference document.'
        }
        $calls = $null
        $result = $reference
        if ($reference.configuration -eq 'C') {
            if ($reference.evidence_basis -ne 'same_replay' -or -not $reference.trace_file) { throw 'C requires a recorded replay MCP session.' }
            $tracePath = if ([IO.Path]::IsPathRooted($reference.trace_file)) { $reference.trace_file } else { Join-Path $ReferenceDirectory $reference.trace_file }
            $trace = Read-EvalJson $tracePath
            if ($trace.mode -ne 'mcp_replay_trace') { throw 'Expected an MCP replay trace for C.' }
            foreach ($key in @('scenario_sha256', 'tool_registry_sha256', 'question', 'sharing')) {
                if ($reference.$key -cne $trace.$key) { throw 'A C reference differs from its recorded MCP identity.' }
            }
            $result = $trace
            $calls = @($trace.calls)
        } elseif ($reference.evidence_basis -ne 'capture_time' -or $null -eq $reference.started_at_unix_ms -or
            @($reference.evidence_files).Count -eq 0) {
            throw 'D/E require dated capture-time evidence; they cannot replay an old live PC.'
        }
        $harnessTest = $reference.harness_test -eq $true -and $catalog.ContainsKey([string]$reference.scenario_sha256) -and
            $catalog[$reference.scenario_sha256].source.origin -eq 'synthetic_fixture'
        Assert-EvalReferenceModel $reference.model $reference.reasoning_effort ([bool]$harnessTest)
        $entries += [pscustomobject]@{
            configuration = $reference.configuration; model = $reference.model; result = $result
            source_file = $file.FullName; answer = $reference.answer; unverified = $reference.unverified
            calls = $calls; evidence_basis = $reference.evidence_basis; artifacts = @($reference.evidence_files)
            duration_ms = $reference.duration_ms; cost_usd = $reference.cost_usd; reference = $reference
        }
    }
}
if ($entries.Count -eq 0) { throw 'No model answers or external references exist. Offline replay output cannot be graded as a model answer.' }
# Shuffle deterministically, recording the seed privately so the packet can be reproduced.
$random = [Random]::new($Seed)
for ($index = $entries.Count - 1; $index -gt 0; $index--) {
    $other = $random.Next($index + 1)
    $temporary = $entries[$index]; $entries[$index] = $entries[$other]; $entries[$other] = $temporary
}
$mapping = @()
$scores = @()
for ($index = 0; $index -lt $entries.Count; $index++) {
    $entry = $entries[$index]
    $result = $entry.result
    if (-not $catalog.ContainsKey([string]$result.scenario_sha256)) { throw 'An answer refers to a scenario outside this campaign.' }
    $context = $catalog[$result.scenario_sha256]
    if ($result.question -cne $context.preview.question -or $result.sharing -notin @('complete', 'anonymized')) {
        throw 'An answer question or sharing mode does not match its recorded scenario.'
    }
    if ($entry.evidence_basis -eq 'same_replay' -and $result.tool_registry_sha256 -cne $context.preview.tool_registry_sha256) {
        throw 'An answer uses a different tool registry. Keep it in a separate campaign.'
    }
    $label = 'answer-{0:D4}' -f ($index + 1)
    $available = Get-Preview $context.source $result.sharing $null
    $observed = if ($null -ne $entry.calls -and $entry.calls.Count -gt 0) { Get-Preview $context.source $result.sharing $entry.calls } else { $null }
    if ($available.sharing -cne $result.sharing) { throw 'The declared sharing mode cannot be reconstructed from this capture.' }
    $groundTruth = $truth[$result.scenario_sha256]
    $artifactNames = @()
    $artifactIndex = 0
    foreach ($artifact in @($entry.artifacts)) {
        if ($null -eq $artifact) { continue }
        if ($artifact.kind -notin @('commands', 'screenshot')) { throw 'Reference evidence must be commands or a screenshot.' }
        $artifactPath = if ([IO.Path]::IsPathRooted($artifact.path)) { $artifact.path } else { Join-Path $ReferenceDirectory $artifact.path }
        $artifactFile = Get-Item -LiteralPath $artifactPath
        if ($artifactFile.PSIsContainer -or $artifactFile.Length -gt 32MB) { throw 'Reference evidence exceeds the file limit.' }
        $artifactIndex++
        $artifactName = '{0}-evidence-{1:D2}{2}' -f $label, $artifactIndex, $artifactFile.Extension
        [IO.File]::Copy($artifactFile.FullName, (Join-Path $grader $artifactName), $false)
        $artifactNames += [pscustomobject]@{ file = $artifactName; kind = $artifact.kind; sha256 = (Get-FileHash -LiteralPath $artifactFile.FullName).Hash.ToLowerInvariant() }
    }
    Write-EvalJson (Join-Path $grader "$label.json") ([ordered]@{
        schema_version = 1; label = $label; question = $result.question; answer = $entry.answer
        unverified_passages = $entry.unverified; ground_truth = $groundTruth.ground_truth
        ground_truth_limitations = $groundTruth.limitations
        available_evidence = [ordered]@{ initial_message = $available.initial_message; calls = $available.calls }
        observed_evidence = if ($null -ne $observed) { [ordered]@{ initial_message = $observed.initial_message; calls = $observed.calls } } else { $null }
        additional_evidence = $artifactNames
    })
    $complete = $entry.evidence_basis -eq 'same_replay' -and $result.replay_complete -eq $true
    if ($entry.configuration -eq 'C') { $complete = $complete -and $result.transport_complete -eq $true -and $result.recording_complete -eq $true }
    $mapping += [pscustomobject]@{
        label = $label; configuration = $entry.configuration; model = $entry.model
        reasoning_effort = $entry.reference.reasoning_effort
        source_file = $entry.source_file; source_sha256 = (Get-FileHash -LiteralPath $entry.source_file).Hash.ToLowerInvariant()
        scenario_sha256 = $result.scenario_sha256; tool_registry_sha256 = $result.tool_registry_sha256
        scenario_origin = $context.source.origin; question_sha256 = Get-EvalTextHash $result.question
        sharing = $result.sharing; evidence_basis = $entry.evidence_basis; replay_complete = [bool]$complete
        reused_calls = $result.reused_calls; duration_ms = $entry.duration_ms; cost_usd = $entry.cost_usd
        ground_truth_established = -not [string]::IsNullOrWhiteSpace($groundTruth.ground_truth)
        collection_offset_ms = if ($entry.evidence_basis -eq 'capture_time') { [long]$entry.reference.started_at_unix_ms - [long]$context.source.captured_at_unix_ms } else { $null }
        packet_sha256 = (Get-FileHash -LiteralPath (Join-Path $grader "$label.json")).Hash.ToLowerInvariant()
    }
    $scores += [pscustomobject]@{
        label = $label; grader_type = 'human'; grader = ''; grader_model = ''; grader_reasoning_effort = ''
        cause = ''; actions = ''; safety = ''; fidelity = ''; clarity = ''
        unsafe_recommendation = ''; unmarked_invented_number = ''; missing_capture = ''; notes = ''
    }
}
Write-EvalJson (Join-Path $private 'mapping.json') ([ordered]@{
    schema_version = 1; seed = $Seed; campaign_manifest_sha256 = (Get-FileHash -LiteralPath (Join-Path $CampaignDirectory 'manifest.json')).Hash.ToLowerInvariant()
    packet_evaluator_sha256 = (Get-FileHash -LiteralPath $Evaluator).Hash.ToLowerInvariant(); answers = $mapping
    excluded_model_runs = $excludedRuns
    campaign_real_scenarios = @($catalog.Keys | Where-Object { $catalog[$_].source.origin -eq 'live' } | Sort-Object)
    references_absent = @(@('C', 'D', 'E') | Where-Object { $_ -notin @($entries.configuration) })
})
$scores | Export-Csv -LiteralPath (Join-Path $grader 'scores.csv') -NoTypeInformation -Encoding UTF8
[IO.File]::Copy((Join-Path $PSScriptRoot '..\eval\rubric.md'), (Join-Path $grader 'rubric.md'), $false)
Write-Host "Prepared $($entries.Count) blinded answers. Give the grader only the grader directory; keep private separate. No judge was contacted."
