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
        # The pinned Cargo predates rust-lang/cargo#17527: cargo test itself
        # introduces a Job that denies explicit child breakaway. Run the exact
        # Cargo-built executables directly, outside that additional Job.
        $code = 0
        foreach ($test in $configuration.tests) {
            if ($test.required) {
                $names = & $test.executable --list
                if ($LASTEXITCODE -ne 0 -or $names -notcontains "$($test.required): test") {
                    throw "Missing native acceptance test: $($test.required)"
                }
            }
            "Running $($test.executable) $($test.arguments -join ' ')" | Add-Content -LiteralPath $configuration.log
            $testArguments = @($test.arguments)
            & $test.executable @testArguments *>> $configuration.log
            $code = $LASTEXITCODE
            if ($code -ne 0) { break }
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
    $directory = (Resolve-Path '../pichi').Path
    $buildLog = Join-Path $root 'build.log'
    Push-Location -LiteralPath $directory
    try {
        $messages = & cargo +nightly-2026-07-06 test --locked -p pichi --lib --test cmd_workload --test cmd_build --test cmd_run --test cmd_rmi --no-run --message-format=json 2> $buildLog
        if ($LASTEXITCODE -ne 0) { throw 'Build of native Windows caller tests failed' }
    } finally {
        Pop-Location
        if (Test-Path -LiteralPath $buildLog) { Get-Content -LiteralPath $buildLog }
    }
    $artifacts = @($messages | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object {
        $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable -and
        ($_.target.kind -contains 'test' -or $_.target.kind -contains 'lib')
    })
    if ($artifacts.Count -ne 5) { throw "Expected five caller test executables, got $($artifacts.Count)" }
    $tests = @($artifacts | ForEach-Object { @{ executable = $_.executable; arguments = @() } })
    $unit = @($artifacts | Where-Object { $_.target.name -eq 'pichi' -and $_.target.kind -contains 'lib' })
    if ($unit.Count -ne 1) { throw 'Cannot identify the caller library test executable' }
    $tests += @{
        executable = $unit[0].executable
        arguments = @('native_monitor_launch_and_async_control', '--ignored', '--nocapture')
        required = 'cmd::workload::tests::native_monitor_launch_and_async_control'
    }
    @{
        environment = $environment
        directory = $directory
        result = $resultFile
        log = $logFile
        tests = $tests
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $requestFile
    $powershell = (Get-Process -Id $PID).Path
    $command = '"{0}" -NoLogo -NoProfile -NonInteractive -File "{1}" -Request "{2}"' -f $powershell, $PSCommandPath, $requestFile
    # WMI avoids the invoking runner's Job, but also has its own provider Job
    # limits unless this documented startup flag is explicitly supplied.
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ CreateFlags = [uint32]0x01000000 }
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $command; ProcessStartupInformation = $startup }
    if ($created.ReturnValue -ne 0) { throw "WMI test-host creation failed: $($created.ReturnValue)" }
    $worker = Get-Process -Id $created.ProcessId
    $null = $worker.Handle
    if (-not $worker.WaitForExit(900000)) { throw 'Windows caller qualification exceeded 15 minutes' }
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
    if (Test-Path -LiteralPath (Join-Path $root 'caller.log')) { Get-Content -LiteralPath (Join-Path $root 'caller.log') }
    Remove-Item -LiteralPath $root -Recurse -Force
}
