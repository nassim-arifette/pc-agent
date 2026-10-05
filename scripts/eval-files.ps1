# Shared local-file helpers for evaluation scripts. No provider calls or credential access.
$script:EvalMaximumBytes = 32MB

function Read-EvalJson([string]$Path) {
    $file = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($file.PSIsContainer -or $file.Length -gt $script:EvalMaximumBytes) {
        throw 'Evaluation input must be a JSON file of at most 32 MiB.'
    }
    try { Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 | ConvertFrom-Json }
    catch { throw 'Invalid evaluation JSON document.' }
}

function Assert-EvalOutsideGit([string]$Directory) {
    $ancestor = [IO.DirectoryInfo]::new((Resolve-Path -LiteralPath $Directory).Path)
    while ($null -ne $ancestor) {
        if (Test-Path -LiteralPath (Join-Path $ancestor.FullName '.git')) {
            throw 'Evaluation data must stay outside Git repositories.'
        }
        $ancestor = $ancestor.Parent
    }
}

function New-EvalDirectory([string]$Path) {
    $fullPath = [IO.Path]::GetFullPath($Path)
    if (Test-Path -LiteralPath $fullPath) { throw 'Choose a new output directory; evaluation data is never overwritten.' }
    $parent = Split-Path -Parent $fullPath
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { throw 'The output parent directory must exist.' }
    Assert-EvalOutsideGit $parent
    [IO.Directory]::CreateDirectory($fullPath) | Out-Null
    $fullPath
}

function Write-EvalText([string]$Path, [string]$Text) {
    Assert-EvalOutsideGit (Split-Path -Parent ([IO.Path]::GetFullPath($Path)))
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes($Text)
    if ($bytes.Length -gt $script:EvalMaximumBytes) { throw 'Evaluation output exceeds 32 MiB.' }
    $stream = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) }
    finally { $stream.Dispose() }
}

function Write-EvalJson([string]$Path, $Value) {
    Write-EvalText $Path ((ConvertTo-Json -InputObject $Value -Depth 100) + [Environment]::NewLine)
}

function Get-EvalTextHash([string]$Text) {
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try { ([BitConverter]::ToString($algorithm.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text)))).Replace('-', '').ToLowerInvariant() }
    finally { $algorithm.Dispose() }
}

function Assert-EvalReferenceModel([string]$Model, [string]$ReasoningEffort, [bool]$HarnessTest = $false) {
    if ($HarnessTest -and $Model -eq 'synthetic-test-model-c' -and $ReasoningEffort -eq 'synthetic') { return }
    $allowed = @{
        'Claude Sonnet 5.5' = 'medium'
        'gpt-6.1-sol' = 'low'
        'gpt-5.6-terra' = 'high'
    }
    if (-not $allowed.ContainsKey($Model) -or $allowed[$Model] -cne $ReasoningEffort) {
        throw 'Reference/judge models are restricted to Claude Sonnet 5.5 medium, Codex gpt-6.1-sol low or gpt-5.6-terra high.'
    }
}

function Save-EvalSource([string]$OutputDirectory, [string]$Checkout) {
    $paths = @('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'apps/eval', 'crates/agent',
        'crates/shared', 'crates/policy', 'crates/telemetry', 'crates/store', 'crates/inspect',
        'crates/storage', 'crates/diagnostics', 'eval', 'scripts/evaluate.ps1',
        'scripts/eval-files.ps1', 'scripts/eval-capture-calls.ps1', 'scripts/grade.ps1', 'scripts/grade-summary.ps1',
        'scripts/test-eval-grading.ps1')
    $revision = & git -C $Checkout rev-parse HEAD 2>$null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify the script checkout revision.' }
    $patch = @(& git -C $Checkout diff --binary --no-ext-diff HEAD -- @paths)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot record the evaluation source patch.' }
    Write-EvalText (Join-Path $OutputDirectory 'source.patch') ($patch -join "`n")
    $names = @(& git -C $Checkout ls-files --cached --others --exclude-standard -- @paths | Sort-Object -Unique)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate evaluation source files.' }
    $untracked = @(& git -C $Checkout ls-files --others --exclude-standard -- @paths)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify new evaluation sources.' }
    $archive = Join-Path $OutputDirectory 'untracked-source'
    [IO.Directory]::CreateDirectory($archive) | Out-Null
    $files = @(foreach ($name in $names) {
        if ([IO.Path]::GetFileName($name) -like '.env*' -or [IO.Path]::GetFileName($name) -eq 'pc-agent-review.md') { continue }
        $path = Join-Path $Checkout $name
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            $archiveFile = $null
            if ($name -in $untracked) {
                if ((Get-Item -LiteralPath $path).Length -gt $script:EvalMaximumBytes) { throw 'New source file exceeds the archive limit.' }
                $archiveName = $hash + [IO.Path]::GetExtension($name)
                $archiveFile = 'untracked-source/' + $archiveName
                $destination = Join-Path $archive $archiveName
                if (-not (Test-Path -LiteralPath $destination)) { [IO.File]::Copy($path, $destination, $false) }
            }
            [pscustomobject]@{ path = $name; sha256 = $hash; untracked_archive_file = $archiveFile }
        }
    })
    Write-EvalJson (Join-Path $OutputDirectory 'source-files.json') $files
    [pscustomobject]@{
        checkout_revision = [string]$revision
        tracked_patch_sha256 = (Get-FileHash -LiteralPath (Join-Path $OutputDirectory 'source.patch')).Hash.ToLowerInvariant()
        source_files_sha256 = (Get-FileHash -LiteralPath (Join-Path $OutputDirectory 'source-files.json')).Hash.ToLowerInvariant()
        note = 'Checkout provenance describes files beside this script. The executable hash identifies the tested build; its source revision must be confirmed by the builder.'
    }
}
