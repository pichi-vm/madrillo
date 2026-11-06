# Native caller qualification outside the runner's restrictive Job Object.
# WMI is test infrastructure only; the product still uses native inherited
# control and CREATE_BREAKAWAY_FROM_JOB, without a fallback or launch broker.
param([string]$Request)
$ErrorActionPreference = 'Stop'

if ($Request) {
    $configuration = Get-Content -LiteralPath $Request -Raw | ConvertFrom-Json
    foreach ($entry in $configuration.environment.PSObject.Properties) {
        [Environment]::SetEnvironmentVariable($entry.Name, [string]$entry.Value, 'Process')
    }
    $code = 1
    try {
        Set-Location -LiteralPath $configuration.directory
        & cargo +nightly-2026-07-06 test --locked -p pichi --lib --test cmd_workload --test cmd_build --test cmd_run --test cmd_rmi *> $configuration.log
        $code = $LASTEXITCODE
        if ($code -eq 0) {
            & cargo +nightly-2026-07-06 test --locked -p pichi --lib native_monitor_launch_and_async_control -- --ignored --nocapture *>> $configuration.log
            $code = $LASTEXITCODE
        }
    } catch {
        $_ | Out-String | Add-Content -LiteralPath $configuration.log
        $code = 1
    } finally {
        [IO.File]::WriteAllText($configuration.result, [string]$code)
    }
    exit $code
}

$root = Join-Path $env:RUNNER_TEMP ('madrillo-caller-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $root | Out-Null
$worker = $null
try {
    $environment = @{}
    # Explicit non-secret build/test environment; no credential snapshot.
    foreach ($name in @('PATH', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH', 'TEMP', 'TMP',
                        'CARGO_HOME', 'RUSTUP_HOME', 'CARGO_NET_GIT_FETCH_WITH_CLI',
                        'PICHI_MADRILLO', 'MADRILLO_BIN', 'CI', 'GITHUB_ACTIONS',
                        'RUNNER_TRACKING_ID')) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($null -ne $value) { $environment[$name] = $value }
    }
    $requestFile = Join-Path $root 'request.json'
    $resultFile = Join-Path $root 'exit.txt'
    $logFile = Join-Path $root 'caller.log'
    @{
        environment = $environment
        directory = (Resolve-Path '../pichi').Path
        result = $resultFile
        log = $logFile
    } | ConvertTo-Json | Set-Content -LiteralPath $requestFile
    $powershell = (Get-Process -Id $PID).Path
    $command = '"{0}" -NoLogo -NoProfile -NonInteractive -File "{1}" -Request "{2}"' -f $powershell, $PSCommandPath, $requestFile
    # Win32_Process.Create does not inherit the invoking process's Job.
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $command }
    if ($created.ReturnValue -ne 0) { throw "WMI test-host creation failed: $($created.ReturnValue)" }
    $worker = Get-Process -Id $created.ProcessId
    $null = $worker.Handle
    if (-not $worker.WaitForExit(900000)) { throw 'Windows caller qualification exceeded 15 minutes' }
    if (Test-Path -LiteralPath $logFile) { Get-Content -LiteralPath $logFile }
    if (-not (Test-Path -LiteralPath $resultFile)) { throw 'Windows test host exited without a result' }
    $code = [int](Get-Content -LiteralPath $resultFile -Raw)
    if ($code -ne 0) { throw "Windows caller qualification failed with exit $code" }
} finally {
    if ($null -ne $worker) {
        if (-not $worker.HasExited) {
            $worker.Kill($true)
            $worker.WaitForExit()
        }
        $worker.Dispose()
    }
    Remove-Item -LiteralPath $root -Recurse -Force
}
