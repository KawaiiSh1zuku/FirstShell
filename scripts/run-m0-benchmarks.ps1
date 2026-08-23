[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$resultDirectory = Join-Path $repositoryRoot 'target/m0-benchmarks'
New-Item -ItemType Directory -Force -Path $resultDirectory | Out-Null

Push-Location $repositoryRoot
try {
    $rustVersion = (& rustc --version) -join "`n"
    $nodeVersion = (& node --version) -join "`n"

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $ipcOutput = (& cargo test -p firstshell-desktop ipc_channel_throughput_reference_benchmark --release -- --ignored --nocapture 2>&1) -join "`n"
    $ipcExitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousErrorActionPreference
    if ($ipcExitCode -ne 0) { throw "IPC benchmark failed`n$ipcOutput" }

    $sshDuration = Measure-Command {
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $script:sshOutput = (& cargo test -p firstshell-ssh --test russh_spike --release 2>&1) -join "`n"
        $sshExitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorActionPreference
        if ($sshExitCode -ne 0) { throw "SSH benchmark failed`n$script:sshOutput" }
    }

    $result = [ordered]@{
        measuredAtUtc = [DateTime]::UtcNow.ToString('o')
        hardware = 'benchmarks/reference-hardware.json'
        dataset = 'benchmarks/dataset.json'
        rust = $rustVersion
        node = $nodeVersion
        ipcOutput = $ipcOutput
        sshElapsedMilliseconds = [Math]::Round($sshDuration.TotalMilliseconds, 3)
        sshOutput = $sshOutput
    }
    $resultPath = Join-Path $resultDirectory 'latest.json'
    $result | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $resultPath
    Write-Host "M0 benchmark results: $resultPath"
} finally {
    Pop-Location
}
