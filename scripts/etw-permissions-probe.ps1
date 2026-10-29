<#
Controlled native preflight, not the forced-exit acceptance and not an activation switch.
Creates two private ETW sessions and one temporary standard account, grants only
the first session GUID query/control rights, probes QUERY/STOP/ACL removal from
that standard account, then removes only resources this invocation created.
Run in an administrative Windows PowerShell. No providers/global logger ACLs change.
#>
[CmdletBinding()]
param(
    [switch]$Child,
    [string]$SessionName,
    [string]$SessionGuid,
    [string]$SentinelName,
    [string]$SentinelGuid,
    [string]$ResultPath,
    [ValidateSet(0x21, 0x23, 0x31, 0xa1, 0x10021, 0x1f0fff)]
    [uint32]$Rights = 0x21
)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class EtwPermissionProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct Wnode {
        public uint BufferSize, ProviderId;
        public ulong HistoricalContext;
        public long TimeStamp;
        public Guid Guid;
        public uint ClientContext, Flags;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct Properties {
        public Wnode Wnode;
        public uint BufferSize, MinimumBuffers, MaximumBuffers, MaximumFileSize;
        public uint LogFileMode, FlushTimer, EnableFlags;
        public int AgeLimit;
        public uint NumberOfBuffers, FreeBuffers, EventsLost, BuffersWritten;
        public uint LogBuffersLost, RealTimeBuffersLost;
        public IntPtr LoggerThreadId;
        public uint LogFileNameOffset, LoggerNameOffset;
    }
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode)]
    static extern uint StartTraceW(out ulong handle, string name, IntPtr properties);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode)]
    static extern uint ControlTraceW(ulong handle, string name, IntPtr properties, uint control);
    [DllImport("advapi32.dll")]
    public static extern uint EventAccessControl(ref Guid guid, uint operation,
        byte[] sid, uint rights, byte allow);
    [DllImport("advapi32.dll")]
    public static extern uint EventAccessRemove(ref Guid guid);
    static IntPtr Buffer(Guid guid) {
        int size = Marshal.SizeOf(typeof(Properties));
        IntPtr data = Marshal.AllocHGlobal(size + 4096);
        Marshal.Copy(new byte[size + 4096], 0, data, size + 4096);
        Properties p = new Properties();
        p.Wnode.BufferSize = (uint)(size + 4096);
        p.Wnode.Guid = guid;
        p.Wnode.ClientContext = 1;
        p.Wnode.Flags = 0x00020000; // WNODE_FLAG_TRACED_GUID
        p.BufferSize = 64; p.MinimumBuffers = 4; p.MaximumBuffers = 32;
        p.LogFileMode = 0x02000100 | 0x10000000; // system, real-time, no per-CPU buffers
        p.EnableFlags = 0x10000000 | 0x20 | 0x40 | 0x100; // NO_SYSCONFIG, DPC, ISR, disk
        p.FlushTimer = 1;
        p.LoggerNameOffset = (uint)size;
        Marshal.StructureToPtr(p, data, false);
        return data;
    }
    public static uint Start(string name, Guid guid) {
        IntPtr data = Buffer(guid);
        try { ulong handle; return StartTraceW(out handle, name, data); }
        finally { Marshal.FreeHGlobal(data); }
    }
    public static uint Control(string name, Guid guid, uint control) {
        IntPtr data = Buffer(guid);
        try {
            if (control == 1) {
                uint query = ControlTraceW(0, name, data, 0);
                if (query != 0) return query;
                Properties found = (Properties)Marshal.PtrToStructure(data, typeof(Properties));
                if (found.Wnode.Guid != guid) return 13;
            }
            return ControlTraceW(0, name, data, control);
        } finally { Marshal.FreeHGlobal(data); }
    }
}
'@

if ($Child) {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $guid = [guid]$SessionGuid
    $result = [ordered]@{
        standard_account = -not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        own_query = [EtwPermissionProbe]::Control($SessionName, $guid, 0)
        unrelated_query = [EtwPermissionProbe]::Control($SentinelName, [guid]$SentinelGuid, 0)
        unrelated_stop = [EtwPermissionProbe]::Control($SentinelName, [guid]$SentinelGuid, 1)
        own_stop = [EtwPermissionProbe]::Control($SessionName, $guid, 1)
        own_remove_permission = [EtwPermissionProbe]::EventAccessRemove([ref]$guid)
    }
    $result | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding UTF8
    exit 0
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run the controller as administrator; it creates a temporary standard-account child.'
}
$id = [guid]::NewGuid().ToString('N')
$account = 'etw' + $id.Substring(0, 12)
$guid = [guid]::NewGuid()
$sentinel = [guid]::NewGuid()
$name = 'pcagent-acceptance-' + $id
$sentinelName = 'pcagent-sentinel-' + $id
$directory = Join-Path $env:PUBLIC ('pcagent-etw-probe-' + $id)
$created = $false
$started = $false
$sentinelStarted = $false
$grant = $false
$process = $null
$result = $null
try {
    $random = New-Object byte[] 32
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    $rng.GetBytes($random)
    $rng.Dispose()
    $secure = ConvertTo-SecureString ('aA1!' + [Convert]::ToBase64String($random)) -AsPlainText -Force
    [Array]::Clear($random, 0, $random.Length)
    $user = New-LocalUser -Name $account -Password $secure -AccountNeverExpires -UserMayNotChangePassword
    $created = $true
    $users = Get-LocalGroup -SID 'S-1-5-32-545'
    Add-LocalGroupMember -Group $users -Member $user
    New-Item -ItemType Directory -Path $directory | Out-Null
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $rule = New-Object Security.AccessControl.FileSystemAccessRule(
            (New-Object Security.Principal.SecurityIdentifier($sid)),
            'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
    }
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
        $user.SID, 'ReadAndExecute', 'ContainerInherit,ObjectInherit', 'None', 'Allow')))
    Set-Acl -LiteralPath $directory -AclObject $acl
    $script = Join-Path $directory 'probe.ps1'
    Copy-Item -LiteralPath $PSCommandPath -Destination $script
    $output = Join-Path $directory 'output'
    New-Item -ItemType Directory -Path $output | Out-Null
    $outputAcl = Get-Acl -LiteralPath $output
    $outputAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
        $user.SID, 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow')))
    Set-Acl -LiteralPath $output -AclObject $outputAcl
    $sidBytes = New-Object byte[] $user.SID.BinaryLength
    $user.SID.GetBinaryForm($sidBytes, 0)
    $code = [EtwPermissionProbe]::EventAccessControl([ref]$guid, 2, $sidBytes, $Rights, 1)
    if ($code -ne 0) { throw "GUID-specific grant failed: $code" }
    $grant = $true
    $code = [EtwPermissionProbe]::Start($name, $guid)
    if ($code -ne 0) { throw "Private StartTrace failed: $code" }
    $started = $true
    $code = [EtwPermissionProbe]::Start($sentinelName, $sentinel)
    if ($code -ne 0) { throw "Private sentinel StartTrace failed: $code" }
    $sentinelStarted = $true
    $alreadyExists = [EtwPermissionProbe]::Start($name, $guid)
    $resultFile = Join-Path $output 'result.json'
    $credential = New-Object Management.Automation.PSCredential(".\$account", $secure)
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$script`"",
        '-Child', '-SessionName', $name, '-SessionGuid', $guid,
        '-SentinelName', $sentinelName, '-SentinelGuid', $sentinel,
        '-ResultPath', "`"$resultFile`"")
    $process = Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList $arguments `
        -Credential $credential -WorkingDirectory $directory -LoadUserProfile -PassThru `
        -RedirectStandardError (Join-Path $output 'error.txt') `
        -RedirectStandardOutput (Join-Path $output 'stdout.txt')
    if (-not $process.WaitForExit(20000)) { throw 'Standard-account child exceeded 20 seconds.' }
    if (-not (Test-Path -LiteralPath $resultFile)) {
        throw 'Standard-account child failed before producing numeric API results.'
    }
    $result = [ordered]@{
        probe = 'GUID-scoped ETW permissions; not guardian/forced-exit acceptance'
        windows_build = [Environment]::OSVersion.Version.Build
        guid_scoped_rights = $Rights
        already_exists = $alreadyExists
        standard = (Get-Content -LiteralPath $resultFile -Raw | ConvertFrom-Json)
        own_after_child = [EtwPermissionProbe]::Control($name, $guid, 0)
        unrelated_after_child = [EtwPermissionProbe]::Control($sentinelName, $sentinel, 0)
    }
} finally {
    if ($process -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
    if ($started) {
        $code = [EtwPermissionProbe]::Control($name, $guid, 1)
        if ($code -notin @(0, 4201)) { Write-Warning "Owned session cleanup failed: $code" }
    }
    if ($sentinelStarted) {
        $code = [EtwPermissionProbe]::Control($sentinelName, $sentinel, 1)
        if ($code -notin @(0, 4201)) { Write-Warning "Sentinel cleanup failed: $code" }
    }
    if ($grant) {
        $code = [EtwPermissionProbe]::EventAccessRemove([ref]$guid)
        if ($code -notin @(0, 2)) { Write-Warning "Owned permission removal failed: $code" }
    }
    if ($created) {
        Get-CimInstance Win32_UserProfile -Filter "SID='$($user.SID.Value)'" |
            Remove-CimInstance -ErrorAction SilentlyContinue
        Remove-LocalUser -Name $account
    }
    if (Test-Path -LiteralPath $directory) { Remove-Item -LiteralPath $directory -Recurse -Force }
}
if ($result) { $result | ConvertTo-Json -Depth 4 }
