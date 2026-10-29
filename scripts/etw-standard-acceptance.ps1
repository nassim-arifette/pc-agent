# Controlled disposable Windows acceptance. Creates and removes one standard account.
# Run from an elevated desktop, outside a job that forbids process breakaway.
[CmdletBinding()]
param(
    [string]$TestBinary,
    [string]$EvidencePath = (Join-Path $env:USERPROFILE 'guardian-standard.log'),
    [switch]$RunStandard,
    [string]$Runtime
)
$ErrorActionPreference = 'Stop'
if ($RunStandard) {
    $env:PCAGENT_ETW_ACCEPTANCE_DIRECTORY = $Runtime
    Remove-Item Env:NEBIUS_API_KEY, Env:NEBIUS_MODEL_ID -ErrorAction SilentlyContinue
    $ErrorActionPreference = 'Continue'
    & (Join-Path $Runtime 'elevation-tests.exe') --ignored --exact `
        trace_guardian::tests::standard_peer_acceptance --nocapture
    exit $LASTEXITCODE
}
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run the acceptance coordinator from an elevated desktop PowerShell.'
}
if (-not $TestBinary) { throw 'Pass the freshly compiled pcagent-elevation test executable.' }
$TestBinary = (Resolve-Path $TestBinary).Path

function Read-EtwSecuritySnapshot {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\WMI\Security')
    try {
        $values = [ordered]@{}
        foreach ($name in ($key.GetValueNames() | Sort-Object)) {
            $values[$name] = [Convert]::ToBase64String($key.GetValue($name))
        }
        return ($values | ConvertTo-Json -Compress)
    } finally { $key.Dispose() }
}

$before = Read-EtwSecuritySnapshot
$id = [guid]::NewGuid().ToString('N')
$Runtime = Join-Path $env:ProgramData "pcagent-etw-acceptance-$id"
$account = 'etw' + $id.Substring(0, 12)
$created = $false
$owner = $null
$peer = $null
$secure = $null
$oldSid = $env:PCAGENT_ETW_ACCEPTANCE_SID
$oldDirectory = $env:PCAGENT_ETW_ACCEPTANCE_DIRECTORY
try {
    $random = New-Object byte[] 32
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    $rng.GetBytes($random)
    $rng.Dispose()
    $secure = ConvertTo-SecureString ('aA1!' + [Convert]::ToBase64String($random)) -AsPlainText -Force
    [Array]::Clear($random, 0, $random.Length)
    $user = New-LocalUser -Name $account -Password $secure -AccountNeverExpires -UserMayNotChangePassword
    $created = $true
    Add-LocalGroupMember -Group (Get-LocalGroup -SID 'S-1-5-32-545') -Member $user
    New-Item -ItemType Directory -Path $Runtime | Out-Null
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid),
            'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    }
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        $user.SID, 'ReadAndExecute', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $Runtime -AclObject $acl
    Copy-Item -LiteralPath $TestBinary -Destination (Join-Path $Runtime 'elevation-tests.exe')
    Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $Runtime 'standard.ps1')
    $result = Join-Path $Runtime 'result'
    New-Item -ItemType Directory -Path $result | Out-Null
    $acl = Get-Acl -LiteralPath $result
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        $user.SID, 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $result -AclObject $acl
    $env:PCAGENT_ETW_ACCEPTANCE_SID = $user.SID.Value
    $env:PCAGENT_ETW_ACCEPTANCE_DIRECTORY = $Runtime
    $owner = Start-Process -FilePath (Join-Path $Runtime 'elevation-tests.exe') `
        -ArgumentList @('--ignored', '--exact', 'trace_guardian::tests::standard_owner_acceptance', '--nocapture') `
        -WorkingDirectory $Runtime -PassThru `
        -RedirectStandardOutput (Join-Path $Runtime 'owner.stdout') `
        -RedirectStandardError (Join-Path $Runtime 'owner.stderr')
    $null = $owner.Handle
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path (Join-Path $Runtime 'probe.json')) -and -not $owner.HasExited -and
        [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 20 }
    if (-not (Test-Path (Join-Path $Runtime 'probe.json'))) {
        Get-Content (Join-Path $Runtime 'owner.stderr')
        throw 'Privileged owner did not become ready; activation remains disabled.'
    }
    $credential = [Management.Automation.PSCredential]::new(".\$account", $secure)
    $peer = Start-Process -FilePath "$PSHOME\powershell.exe" `
        -ArgumentList @('-NoProfile', '-File', "`"$(Join-Path $Runtime 'standard.ps1')`"", '-RunStandard', '-Runtime', "`"$Runtime`"") `
        -Credential $credential -LoadUserProfile -WorkingDirectory $Runtime -PassThru `
        -RedirectStandardOutput (Join-Path $result 'peer.stdout') `
        -RedirectStandardError (Join-Path $result 'peer.stderr')
    $null = $peer.Handle
    if (-not $peer.WaitForExit(25000)) { throw 'Standard peer exceeded its deadline.' }
    if (-not $owner.WaitForExit(45000)) { throw 'Privileged owner exceeded its deadline.' }
    $peer.Refresh()
    $owner.Refresh()
    $evidence = @(
        "Windows build: $([Environment]::OSVersion.Version.Build)"
        "Test binary SHA256: $((Get-FileHash $TestBinary -Algorithm SHA256).Hash)"
        "Owner exit code: $($owner.ExitCode); standard peer exit code: $($peer.ExitCode)"
        (Get-Content (Join-Path $Runtime 'owner.stdout'))
        (Get-Content (Join-Path $Runtime 'owner.stderr'))
        (Get-Content (Join-Path $result 'peer.stdout'))
        (Get-Content (Join-Path $result 'peer.stderr'))
    )
    $evidence | Set-Content -LiteralPath $EvidencePath -Encoding UTF8
    $evidence | Write-Output
    if ($peer.ExitCode -ne 0 -or $owner.ExitCode -ne 0) { throw 'Native acceptance failed.' }
    if ((Read-EtwSecuritySnapshot) -cne $before) { throw 'ETW security registry changed after cleanup.' }
    'guardian_standard etw_security_registry_unchanged=true' | Add-Content -LiteralPath $EvidencePath
} finally {
    if ($peer -and -not $peer.HasExited) { $peer.Kill(); $peer.WaitForExit() }
    # Let the owner observe peer failure and clean both its sentinel and guarded session.
    if ($owner -and -not $owner.HasExited -and -not $owner.WaitForExit(45000)) {
        Write-Warning 'Owner did not finish; inspect private-session cleanup before rerunning.'
    }
    $env:PCAGENT_ETW_ACCEPTANCE_SID = $oldSid
    $env:PCAGENT_ETW_ACCEPTANCE_DIRECTORY = $oldDirectory
    if ($created) {
        Get-CimInstance Win32_UserProfile -Filter "SID='$($user.SID.Value)'" |
            Remove-CimInstance -ErrorAction SilentlyContinue
        Remove-LocalUser -Name $account
    }
    if ($secure) { $secure.Dispose() }
    if (Test-Path -LiteralPath $Runtime) { Remove-Item -LiteralPath $Runtime -Recurse -Force }
}
