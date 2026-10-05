# GitHub's hosted Windows runner is elevated with UAC disabled. Compile as the runner,
# then run every Cargo test executable as an ephemeral standard user. The application
# elevation guard stays enabled. This script never creates an account on a local PC.
[CmdletBinding()]
param(
    [switch]$RunTestExecutables,
    [string]$ManifestPath
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if ($RunTestExecutables) {
    if ($env:GITHUB_ACTIONS -ne 'true' -or (Test-Administrator)) {
        throw 'Test executables require the standard-user CI session.'
    }
    $manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
    Set-Location -LiteralPath $manifest.workspace
    $env:USERPROFILE = $manifest.profile
    $env:APPDATA = Join-Path $manifest.profile 'AppData/Roaming'
    $env:LOCALAPPDATA = Join-Path $manifest.profile 'AppData/Local'
    $env:TEMP = $manifest.temp
    $env:TMP = $manifest.temp
    foreach ($directory in @($env:APPDATA, $env:LOCALAPPDATA, $env:TEMP)) {
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
    }
    Remove-Item Env:NEBIUS_API_KEY, Env:NEBIUS_MODEL_ID -ErrorAction SilentlyContinue
    foreach ($executable in $manifest.executables) {
        Write-Host "Running $([IO.Path]::GetFileName($executable)) with a standard-user token"
        & $executable
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    }
    exit 0
}

if (-not $IsWindows) {
    throw 'Use cargo test --workspace --locked on non-Windows hosts.'
}
if (-not (Test-Administrator)) {
    # Local development already uses the intended token and needs no CI account.
    & cargo test --workspace --locked
    exit $LASTEXITCODE
}
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'Account creation is restricted to the disposable GitHub Actions runner.'
}

$workspace = (Resolve-Path -LiteralPath $env:GITHUB_WORKSPACE).Path
Set-Location -LiteralPath $workspace
$messages = & cargo test --workspace --locked --no-run --message-format=json
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
$artifacts = @($messages | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.executable })
$executables = @($artifacts | Where-Object { $_.profile.test } |
    ForEach-Object { $_.executable } | Sort-Object -Unique)
$cli = @($artifacts | Where-Object {
    $_.target.name -eq 'pcagent' -and $_.target.kind -contains 'bin' -and -not $_.profile.test
} | ForEach-Object { $_.executable } | Sort-Object -Unique)
if ($executables.Count -eq 0 -or $cli.Count -ne 1) {
    throw 'Cargo did not provide the complete test executables and CLI binary.'
}

$runtime = Join-Path $env:RUNNER_TEMP ('pcagent-standard-tests-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $runtime | Out-Null

# Also prove that the real elevated runner cannot launch the normal application.
$guardOut = Join-Path $runtime 'elevated-guard.stdout'
$guardErr = Join-Path $runtime 'elevated-guard.stderr'
$guard = Start-Process -FilePath $cli[0] -ArgumentList @('doctor', '--json') `
    -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput $guardOut -RedirectStandardError $guardErr
if ($guard.ExitCode -ne 2 -or (Get-Item -LiteralPath $guardOut).Length -ne 0 -or
    (Get-Content -LiteralPath $guardErr -Raw).Trim() -ne 'Start PC Agent from a standard, unelevated session.') {
    throw 'The application must refuse the elevated runner before collecting or writing stdout.'
}
Write-Host 'The real CLI correctly refuses the elevated runner.'

$accountName = 'pcagent-ci-' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$passwordText = 'Aa1!' + [Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
$password = ConvertTo-SecureString -String $passwordText -AsPlainText -Force
$passwordText = $null
$accountCreated = $false
try {
    $account = New-LocalUser -Name $accountName -Password $password -AccountNeverExpires `
        -PasswordNeverExpires -Description 'Ephemeral PC Agent CI test account'
    $accountCreated = $true
    $users = Get-LocalGroup -SID 'S-1-5-32-545'
    Add-LocalGroupMember -Group $users -Member $account
    $sid = $account.SID.Value
    # Grant access only to this disposable checkout and this run's dedicated directory.
    foreach ($directory in @($workspace, $runtime)) {
        & icacls $directory /grant "*${sid}:(OI)(CI)M" | Out-Host
        if ($LASTEXITCODE -ne 0) {
            throw 'Could not grant the standard test account access to the CI paths.'
        }
    }
    $manifestFile = Join-Path $runtime 'tests.json'
    @{
        workspace = $workspace
        executables = $executables
        profile = Join-Path $runtime 'profile'
        temp = Join-Path $runtime 'temp'
    } | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $manifestFile -Encoding utf8
    $credential = [Management.Automation.PSCredential]::new("$env:COMPUTERNAME\$accountName", $password)
    $stdout = Join-Path $runtime 'tests.stdout'
    $stderr = Join-Path $runtime 'tests.stderr'
    $script = Join-Path $workspace 'scripts/ci-windows-tests.ps1'
    $arguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', "`"$script`"",
        '-RunTestExecutables', '-ManifestPath', "`"$manifestFile`"")
    $process = Start-Process -FilePath (Get-Command pwsh).Source -ArgumentList $arguments `
        -Credential $credential -LoadUserProfile -WorkingDirectory $workspace -WindowStyle Hidden `
        -Wait -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    Get-Content -LiteralPath $stdout | Out-Host
    Get-Content -LiteralPath $stderr | Out-Host
    if ($process.ExitCode -ne 0) {
        throw "Standard-user test executables failed with exit code $($process.ExitCode)."
    }
}
finally {
    if ($accountCreated) {
        Remove-LocalUser -Name $accountName
    }
    $password.Dispose()
}

# Cargo's no-run output does not include doctests; retain their normal validation too.
& cargo test --workspace --doc --locked
exit $LASTEXITCODE
