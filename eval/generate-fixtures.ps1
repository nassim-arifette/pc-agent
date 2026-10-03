<#
.SYNOPSIS
Regenerate the small synthetic scenarios used by offline evaluation and command tests.
.DESCRIPTION
No collector, filesystem scan or network request is used. These are not quality benchmark results.
#>
$ErrorActionPreference = 'Stop'
$scenarioDir = Join-Path $PSScriptRoot 'scenarios'
New-Item -ItemType Directory -Force -Path $scenarioDir | Out-Null
$fixtureTime = 1790899200000
$window = @{ started_at_unix_ms = $fixtureTime; finished_at_unix_ms = $fixtureTime + 1000; duration_ms = 1000 }
$coverage = @{ status = 'unsupported'; enumerated = 0; readable = 0; permission_denied = 0; unavailable = 0; returned = 0; enumeration_truncated = $false }
$cases = @(
    @{ name = 'memory-pressure'; question = 'Why is this 8 GiB PC slow?'; available = 268435456; cpu = 1200; latency = 1000; idle = 9800; note = 'Synthetic low available memory; no trend or causal attribution is established.' },
    @{ name = 'cpu-busy'; question = 'Why is the processor busy?'; available = 4294967296; cpu = 9800; latency = 1000; idle = 9800; note = 'Synthetic high CPU sample; process attribution was not captured.' },
    @{ name = 'disk-latency'; question = 'Why does opening applications take time?'; available = 4294967296; cpu = 1200; latency = 85000; idle = 200; note = 'Synthetic disk latency; no hardware failure can be inferred from one sample.' },
    @{ name = 'no-contention'; question = 'Is there a problem in this sample?'; available = 4294967296; cpu = 1200; latency = 1000; idle = 9800; note = 'Synthetic moderate core metrics; one sample does not rule out an intermittent problem.' },
    @{ name = 'storage-full'; question = 'Why is my system drive almost full?'; available = 4294967296; cpu = 1200; latency = 1000; idle = 9800; note = 'Synthetic volume with little free space; no scan or deletion candidate was captured.' }
)
foreach ($case in $cases) {
    $values = [ordered]@{
        physical_total_bytes = 8589934592
        physical_available_bytes = $case.available
        system_commit_bytes = 6442450944
        system_commit_limit_bytes = 17179869184
        cpu_busy_basis_points = $case.cpu
        cpu_busiest_core_basis_points = $case.cpu
        disk_read_latency_microseconds = $case.latency
        disk_write_latency_microseconds = $case.latency
        disk_idle_basis_points = $case.idle
        disk_queue_length = 0
        hard_fault_pages_per_second_x100 = 0
        gpu_busiest_engine_basis_points = 100
    }
    $units = [ordered]@{}
    $evidence = @()
    foreach ($metric in $values.Keys) {
        $unit = if ($metric.EndsWith('_bytes')) { 'bytes' }
            elseif ($metric.EndsWith('_microseconds')) { 'microseconds' }
            else {
                switch ($metric) {
                    'cpu_busy_basis_points' { 'basis_points_of_total_cpu' }
                    'cpu_busiest_core_basis_points' { 'basis_points_of_one_logical_processor' }
                    'disk_idle_basis_points' { 'basis_points_of_window' }
                    'gpu_busiest_engine_basis_points' { 'basis_points_of_engine_time' }
                    'hard_fault_pages_per_second_x100' { 'pages_per_second_x100' }
                    'disk_queue_length' { 'outstanding_requests' }
                    default { throw "Unknown fixture metric: $metric" }
                }
            }
        $units[$metric] = $unit
        $evidence += @{
            id = "ev-$($case.name)-$metric"; metric = $metric; entity_ref = 'system'
            value = $values[$metric]; unit = $unit; source = 'synthetic evaluation fixture'
            window = $window; status = 'ok'; origin = 'synthetic_fixture'; limitations = @()
        }
    }
    $cloud = @{
        schema_version = 1; origin = 'synthetic_fixture'; measured_at_unix_ms = $fixtureTime + 1000
        measurements = @{ values = @{ system = $values }; units = $units }
        process_coverage = $coverage; limitations = @('Synthetic fixture; not a measurement of a real PC.')
    }
    $calls = @(@{
        tool = 'get_system_snapshot'; arguments = @{}
        output = @{ status = 'ok'; inspection_complete = $true; cloud = $cloud; evidence = $evidence; findings = @(); processes = @(); limitations = @() }
        recorded_at_unix_ms = $fixtureTime; duration_ms = 1000
    })
    $toolNames = @('get_system_snapshot', 'get_processes', 'get_hardware_inventory')
    if ($case.name -eq 'storage-full') {
        $toolNames += @('list_storage_scopes', 'get_storage_overview')
        $calls += @{
            tool = 'list_storage_scopes'; arguments = @{}
            output = @{ status = 'ok'; inspection_complete = $false; cloud = @{ scopes = @(@{ scope_id = 'scope-fixture'; scan_available = $false }) }; evidence = @(); findings = @(); processes = @(); limitations = @() }
        }
        $calls += @{
            tool = 'get_storage_overview'; arguments = @{ scope_id = 'scope-fixture' }
            output = @{ status = 'partial'; inspection_complete = $true; cloud = @{
                scope_id = 'scope-fixture'; scan_running = $false; scan = $null
                volume = @{ status = 'ok'; total_bytes = 274877906944; free_bytes = 1073741824 }
                limitations = @('Volume capacity only; no folder scan.')
            }; evidence = @(); findings = @(); processes = @(); limitations = @() }
        }
    }
    $scenario = [ordered]@{
        schema_version = 1; name = "synthetic-$($case.name)"; question = $case.question
        captured_at_unix_ms = $fixtureTime
        initial = @{
            schema_version = 1; id = "snapshot-$($case.name)"; session_id = 'synthetic-fixture'
            origin = 'synthetic_fixture'; evidence = $evidence; processes = @(); process_coverage = $coverage
            limitations = @('Synthetic fixture; not a measurement of a real PC.')
        }
        calls = $calls; notes = @($case.note); sharing = 'anonymized'; tool_names = $toolNames
    }
    $json = $scenario | ConvertTo-Json -Depth 30
    [System.IO.File]::WriteAllText((Join-Path $scenarioDir "$($case.name).json"), $json + "`n", [System.Text.UTF8Encoding]::new($false))
}
