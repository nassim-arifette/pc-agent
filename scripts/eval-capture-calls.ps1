<#
.SYNOPSIS
Extract the union of exact missing calls from replay results for a new offline capture.
.DESCRIPTION
Does not collect, call a provider or change any previous scenario. Pass the saved list to capture
--extra-calls, then run all configurations against the resulting new frozen scenario.
#>
param(
    [Parameter(Mandatory)][string[]]$Results,
    [Parameter(Mandatory)][string]$Output
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'eval-files.ps1')
$unique = @{}
foreach ($path in $Results) {
    $result = Read-EvalJson $path
    if ($result.mode -notin @('nebius_replay', 'offline_replay', 'mcp_replay_trace')) { throw 'Expected a replay result with explicit misses.' }
    foreach ($miss in @($result.replay_misses)) {
        if ($null -eq $miss) { continue }
        if (-not $miss.tool -or $null -eq $miss.arguments -or $miss.arguments -isnot [pscustomobject]) {
            throw 'Invalid recorded missing call.'
        }
        # Current closed schemas use flat argument objects. Sort keys for equivalent JSON order;
        # no value, default, limit or process reference is changed.
        $arguments = [ordered]@{}
        foreach ($property in @($miss.arguments.PSObject.Properties | Sort-Object Name)) {
            $arguments[$property.Name] = $property.Value
        }
        $call = [pscustomobject]@{ tool = [string]$miss.tool; arguments = $arguments }
        $key = ConvertTo-Json -InputObject $call -Compress -Depth 100
        $unique[$key] = $call
    }
}
$calls = @($unique.Keys | Sort-Object | ForEach-Object { $unique[$_] })
if ($calls.Count -eq 0 -or $calls.Count -gt 4096) { throw 'Expected between 1 and 4096 distinct missing calls.' }
Write-EvalJson $Output $calls
Write-Host "Saved $($calls.Count) exact call variants. Capture them in a new situation; do not append later measurements to an old scenario."
