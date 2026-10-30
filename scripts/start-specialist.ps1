param(
    [ValidateSet('cpu', 'cuda')][string]$Device = 'cpu',
    [switch]$Offline,
    [switch]$SetupOnly,
    [string]$Archive,
    [string]$Cache = "$env:LOCALAPPDATA\pcagent\specialist"
)
$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot '..\experiments\diagnostic-specialist'
$python = Join-Path $Cache 'venv\Scripts\python.exe'
$bundle = Join-Path $Cache 'Llama-PC-Agent-Nano4B-Specialist-v0.1.0'
if (!$Offline) {
    New-Item -ItemType Directory -Force $Cache | Out-Null
    if (!(Test-Path $python)) {
        python -m venv (Join-Path $Cache 'venv')
        if ($LASTEXITCODE -ne 0) { throw 'Python 3.10+ is required.' }
    }
    $index = if ($Device -eq 'cuda') { 'https://download.pytorch.org/whl/cu126' } else { 'https://download.pytorch.org/whl/cpu' }
    & $python -m pip install 'torch==2.7.1' --index-url $index
    if ($LASTEXITCODE -ne 0) { throw 'PyTorch installation failed.' }
    & $python -m pip install -r (Join-Path $source 'requirements-local.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Local inference dependencies could not be installed.' }
    $downloadArguments = @((Join-Path $source 'download_adapter.py'), '--directory', $Cache)
    if ($Archive) { $downloadArguments += @('--archive', $Archive) }
    & $python @downloadArguments
    if ($LASTEXITCODE -ne 0) { throw 'Adapter integrity verification failed.' }
}
if ($SetupOnly) { return }
if (!(Test-Path $python) -or !(Test-Path (Join-Path $bundle 'adapter\adapter_model.safetensors'))) {
    throw 'Run once without -Offline to install the dependencies and adapter.'
}
$arguments = @((Join-Path $source 'local.py'), '--adapter', (Join-Path $bundle 'adapter'), '--device', $Device)
if ($Device -eq 'cuda') {
    & $python -m pip show bitsandbytes *> $null
    if ($LASTEXITCODE -ne 0 -and !$Offline) {
        & $python -m pip install 'bitsandbytes==0.45.5'
        if ($LASTEXITCODE -ne 0) { throw 'NF4 dependencies could not be installed.' }
    }
    $arguments += '--nf4'
}
if ($Offline) {
    $env:HF_HUB_OFFLINE = '1'
    $env:TRANSFORMERS_OFFLINE = '1'
    $arguments += '--offline'
}
$env:HF_HUB_DISABLE_TELEMETRY = '1'
& $python @arguments
exit $LASTEXITCODE
