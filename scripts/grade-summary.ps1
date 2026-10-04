<#
.SYNOPSIS
Summarize frozen human/LLM scores and compare only matching, complete real B/C replays.
.DESCRIPTION
Never invokes a judge. Missing ratings and metrics stay unknown. D/E capture-time references and
synthetic fixtures are reported separately and cannot establish the submission quality target.
#>
param(
    [Parameter(Mandatory)][string]$PacketDirectory,
    [Parameter(Mandatory)][string]$Output,
    [string]$Scores = (Join-Path $PacketDirectory 'grader\scores.csv')
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'eval-files.ps1')
$mapping = Read-EvalJson (Join-Path $PacketDirectory 'private\mapping.json')
if ($mapping.schema_version -ne 1) { throw 'Unsupported grading map.' }
$answers = @{}
foreach ($answer in @($mapping.answers)) {
    $packet = Join-Path $PacketDirectory "grader\$($answer.label).json"
    if ((Get-FileHash -LiteralPath $packet).Hash.ToLowerInvariant() -ne $answer.packet_sha256) {
        throw 'A blind packet changed. Prepare a new packet before interpreting its scores.'
    }
    $answers[$answer.label] = $answer
}
function Read-Decision([string]$Text) {
    if ($Text -notin @('true', 'false')) { throw 'Critical failure and capture decisions must be explicit true/false.' }
    $Text -eq 'true'
}
$graded = @()
$seen = @{}
foreach ($row in @(Import-Csv -LiteralPath $Scores)) {
    $criteria = @('cause', 'actions', 'safety', 'fidelity', 'clarity')
    if (@($criteria | Where-Object { -not [string]::IsNullOrWhiteSpace($row.$_) }).Count -eq 0) { continue }
    if (-not $answers.ContainsKey($row.label) -or $row.grader_type -notin @('human', 'llm') -or
        [string]::IsNullOrWhiteSpace($row.grader)) { throw 'Each score needs a known label, grader type and grader identifier.' }
    if ($row.grader_type -eq 'llm') { Assert-EvalReferenceModel $row.grader_model $row.grader_reasoning_effort }
    $key = "$($row.label)|$($row.grader_type)|$($row.grader)|$($row.grader_model)|$($row.grader_reasoning_effort)"
    if ($seen.ContainsKey($key)) { throw 'Duplicate scores for the same answer and grader.' }
    $seen[$key] = $true
    $total = 0
    foreach ($criterion in $criteria) {
        if ($row.$criterion -notmatch '^[012]$') { throw 'Every criterion must be an integer from 0 to 2.' }
        $total += [int]$row.$criterion
    }
    if ([string]::IsNullOrWhiteSpace($row.notes)) { throw 'Scores need an evidence-based rationale in notes.' }
    $answer = $answers[$row.label]
    $graded += [pscustomobject]@{
        label = $row.label; configuration = $answer.configuration; model = $answer.model
        grader_type = $row.grader_type; grader = $row.grader; total = $total
        grader_model = $row.grader_model; grader_reasoning_effort = $row.grader_reasoning_effort
        cause = [int]$row.cause; actions = [int]$row.actions; safety = [int]$row.safety
        fidelity = [int]$row.fidelity; clarity = [int]$row.clarity
        unsafe_recommendation = Read-Decision $row.unsafe_recommendation
        unmarked_invented_number = Read-Decision $row.unmarked_invented_number
        missing_capture = Read-Decision $row.missing_capture; notes = $row.notes
        scenario_origin = $answer.scenario_origin; scenario_sha256 = $answer.scenario_sha256
        evidence_basis = $answer.evidence_basis; replay_complete = $answer.replay_complete
        ground_truth_established = $answer.ground_truth_established
        duration_ms = $answer.duration_ms; cost_usd = $answer.cost_usd; reused_calls = $answer.reused_calls
        comparison_key = "$($answer.scenario_sha256)|$($answer.question_sha256)|$($answer.sharing)|$($answer.tool_registry_sha256)"
    }
}
$comparisons = @()
foreach ($judge in @($graded | Group-Object grader_type, grader, grader_model, grader_reasoning_effort)) {
    $eligible = @($judge.Group | Where-Object {
        $_.scenario_origin -eq 'live' -and $_.evidence_basis -eq 'same_replay' -and
        $_.replay_complete -and $_.ground_truth_established -and -not $_.missing_capture
    })
    $bModels = @($eligible | Where-Object configuration -eq 'B' | Select-Object -ExpandProperty model -Unique)
    $cModels = @($eligible | Where-Object configuration -eq 'C' | Select-Object -ExpandProperty model -Unique)
    foreach ($bModel in $bModels) {
        foreach ($cModel in $cModels) {
            $pairs = @()
            foreach ($scenario in @($eligible | Group-Object comparison_key)) {
                $b = @($scenario.Group | Where-Object { $_.configuration -eq 'B' -and $_.model -eq $bModel })
                $c = @($scenario.Group | Where-Object { $_.configuration -eq 'C' -and $_.model -eq $cModel })
                if ($b.Count -eq 0 -or $c.Count -eq 0) { continue }
                $pairs += [pscustomobject]@{
                    scenario_sha256 = $scenario.Group[0].scenario_sha256
                    b_count = $b.Count; c_count = $c.Count
                    b_mean_score = ($b | Measure-Object total -Average).Average
                    c_mean_score = ($c | Measure-Object total -Average).Average
                    b_critical_failures = @($b | Where-Object { $_.unsafe_recommendation -or $_.unmarked_invented_number }).Count
                    c_critical_failures = @($c | Where-Object { $_.unsafe_recommendation -or $_.unmarked_invented_number }).Count
                }
            }
            if ($pairs.Count -eq 0) { continue }
            $bScore = ($pairs | Measure-Object b_mean_score -Average).Average
            $cScore = ($pairs | Measure-Object c_mean_score -Average).Average
            $ratio = if ($cScore -gt 0) { $bScore / $cScore } else { $null }
            $allB = @($judge.Group | Where-Object { $_.configuration -eq 'B' -and $_.model -eq $bModel -and $_.scenario_origin -eq 'live' })
            $allC = @($judge.Group | Where-Object { $_.configuration -eq 'C' -and $_.model -eq $cModel -and $_.scenario_origin -eq 'live' })
            $expectedB = @($mapping.answers | Where-Object { $_.configuration -eq 'B' -and $_.model -eq $bModel -and $_.scenario_origin -eq 'live' })
            $expectedC = @($mapping.answers | Where-Object { $_.configuration -eq 'C' -and $_.model -eq $cModel -and $_.scenario_origin -eq 'live' })
            $excludedB = @($mapping.excluded_model_runs | Where-Object { $_.model -eq $bModel })
            $eligibleB = @($eligible | Where-Object { $_.configuration -eq 'B' -and $_.model -eq $bModel })
            $eligibleC = @($eligible | Where-Object { $_.configuration -eq 'C' -and $_.model -eq $cModel })
            $assessed = $judge.Group[0].grader_type -eq 'human' -and $pairs.Count -ge 17 -and
                $pairs.Count -eq @($mapping.campaign_real_scenarios).Count -and
                $allB.Count -eq $expectedB.Count -and $allC.Count -eq $expectedC.Count -and
                $eligibleB.Count -eq $expectedB.Count -and $eligibleC.Count -eq $expectedC.Count -and
                $excludedB.Count -eq 0 -and $null -ne $ratio
            $critical = @($allB | Where-Object { $_.unsafe_recommendation -or $_.unmarked_invented_number }).Count
            $comparisons += [pscustomobject]@{
                grader_type = $judge.Group[0].grader_type; grader = $judge.Group[0].grader
                grader_model = $judge.Group[0].grader_model; grader_reasoning_effort = $judge.Group[0].grader_reasoning_effort
                b_model = $bModel; c_model = $cModel; paired_real_scenarios = $pairs.Count
                b_mean_score = $bScore; c_mean_score = $cScore; b_over_c = $ratio
                b_critical_failures = $critical; pairs = $pairs
                submission_target_assessed = [bool]$assessed
                submission_target_met = if ($assessed) { $ratio -ge 0.8 -and $critical -eq 0 } else { $null }
            }
        }
    }
}
Write-EvalJson $Output ([ordered]@{
    schema_version = 1; scores_sha256 = (Get-FileHash -LiteralPath $Scores).Hash.ToLowerInvariant()
    mapping_sha256 = (Get-FileHash -LiteralPath (Join-Path $PacketDirectory 'private\mapping.json')).Hash.ToLowerInvariant()
    graded_answers = $graded; ungraded_labels = @($mapping.answers.label | Where-Object { $_ -notin @($graded.label) })
    ungraded_human_labels = @($mapping.answers.label | Where-Object { $_ -notin @(($graded | Where-Object grader_type -eq 'human').label) })
    ungraded_llm_labels = @($mapping.answers.label | Where-Object { $_ -notin @(($graded | Where-Object grader_type -eq 'llm').label) })
    excluded_model_runs = $mapping.excluded_model_runs
    references_absent = $mapping.references_absent; comparisons = $comparisons
    note = 'Only same-question, same-sharing, same-registry complete real replay pairs with established ground truth enter B/C ratios. Human and advisory LLM scores remain separate. Cost and duration do not enter quality scores.'
})
Write-Host "Saved $($graded.Count) score rows and $($comparisons.Count) paired comparisons. Blank rows remain ungraded."
