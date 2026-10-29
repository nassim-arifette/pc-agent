# Run from an elevated desktop PowerShell, not a CI job that forbids breakaway.
# Never changes global ETW security, group membership or provider permissions.
[CmdletBinding()]
param(
    [string]$TestBinary,
    [string]$EvidencePath = (Join-Path $env:USERPROFILE 'guardian-acceptance.log')
)
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run the acceptance coordinator from an elevated PowerShell.'
}
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = 'C:\pcagent-target' }
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    if (-not $TestBinary) {
        $build = & cargo test -p pcagent-elevation --locked --no-run --message-format=json
        if ($LASTEXITCODE -ne 0) { throw 'Acceptance binary build failed.' }
        $artifacts = @($build | ForEach-Object {
            $record = $_ | ConvertFrom-Json
            if ($record.reason -eq 'compiler-artifact' -and $record.target.name -eq 'pcagent_elevation' -and $record.profile.test -and $record.executable) {
                $record.executable
            }
        })
        if ($artifacts.Count -ne 1) { throw 'Expected exactly one freshly built elevation test binary.' }
        $TestBinary = $artifacts[0]
    }
    $TestBinary = (Resolve-Path $TestBinary).Path
    $env:PCAGENT_ETW_ACCEPTANCE_SID = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $ErrorActionPreference = 'Continue'
    $output = & $TestBinary --ignored --exact trace_guardian::tests::native_acceptance --nocapture 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $output | Tee-Object -FilePath $EvidencePath
    if ($exitCode -ne 0 -or -not ($output -match '1 passed; 0 failed')) {
        throw 'Guardian acceptance failed. Production activation remains disabled.'
    }
} finally {
    Remove-Item Env:\PCAGENT_ETW_ACCEPTANCE_SID -ErrorAction SilentlyContinue
    Pop-Location
}
