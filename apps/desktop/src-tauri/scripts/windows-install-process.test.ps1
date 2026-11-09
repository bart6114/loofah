$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-install-process.ps1"

class FakeInstallProcess {
    [System.Collections.Generic.List[string]]$Calls = @()
    [bool]$HasExited = $false
    [int]$ExitCode = 0
    [bool]$TimeoutFirst = $false
    [bool]$ThrowFirstWait = $false
    [bool]$ThrowKill = $false
    [bool]$ThrowDispose = $false
    [bool]$NeverExits = $false
    [int]$Waits = 0

    [bool] WaitForExit([int]$timeout) {
        $this.Calls.Add("wait:$timeout")
        $this.Waits++
        if ($this.NeverExits) { return $false }
        if ($this.Waits -eq 1 -and $this.ThrowFirstWait) { throw 'original wait failure' }
        if ($this.Waits -eq 1 -and $this.TimeoutFirst) { return $false }
        $this.HasExited = $true
        return $true
    }
    [void] Kill() {
        $this.Calls.Add('kill')
        if ($this.ThrowKill) { throw 'cleanup kill failure' }
    }
    [void] Dispose() {
        $this.Calls.Add('dispose')
        if ($this.ThrowDispose) { throw 'cleanup disposal failure' }
    }
}

function Assert-Calls($process, $expected) {
    $actual = $process.Calls -join ','
    if ($actual -ne $expected) { throw "Expected $expected; observed $actual" }
}
function Assert-Failure($action, $message) {
    $failure = $null
    try { & $action } catch { $failure = $_.Exception.Message }
    if ($null -eq $failure -or -not $failure.Contains($message)) {
        throw "Expected failure '$message'; observed '$failure'"
    }
}

$process = [FakeInstallProcess]::new()
Complete-OwnedInstallProcess $process -RequireZeroExit
Assert-Calls $process 'wait:180000,dispose'

$process = [FakeInstallProcess]::new()
Complete-OwnedInstallProcess $process -Stop
Assert-Calls $process 'kill,wait:180000,dispose'

$process = [FakeInstallProcess]::new()
$process.HasExited = $true
Complete-OwnedInstallProcess $process -Stop
Assert-Calls $process 'wait:180000,dispose'

$process = [FakeInstallProcess]::new()
$process.ExitCode = 7
$process.ThrowDispose = $true
Assert-Failure { Complete-OwnedInstallProcess $process -RequireZeroExit } 'Installer exited with 7'
Assert-Calls $process 'wait:180000,dispose'

$process = [FakeInstallProcess]::new()
$process.TimeoutFirst = $true
Assert-Failure { Complete-OwnedInstallProcess $process -TimeoutMs 1 } 'Owned installer test process timed out'
Assert-Calls $process 'wait:1,kill,wait:30000,dispose'

$process = [FakeInstallProcess]::new()
$process.ThrowFirstWait = $true
Assert-Failure { Complete-OwnedInstallProcess $process } 'original wait failure'
Assert-Calls $process 'wait:180000,kill,wait:30000,dispose'

$process = [FakeInstallProcess]::new()
$process.ThrowKill = $true
$process.ThrowDispose = $true
Assert-Failure { Complete-OwnedInstallProcess $process -Stop -PrimaryError 'original startup failure' } 'original startup failure'
Assert-Calls $process 'kill,wait:30000,dispose'

$process = [FakeInstallProcess]::new()
$process.NeverExits = $true
Assert-Failure { Complete-OwnedInstallProcess $process -TimeoutMs 1 } 'Owned installer test process timed out'
Assert-Calls $process 'wait:1,kill,wait:30000,dispose'

$process = [FakeInstallProcess]::new()
$process.ThrowDispose = $true
Assert-Failure { Complete-OwnedInstallProcess $process } 'cleanup disposal failure'
Assert-Calls $process 'wait:180000,dispose'

Write-Output 'Owned installer process tests passed: 9 cases; no installer or application executed.'

. "$PSScriptRoot/windows-install-diagnostics.ps1"
$fixture = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('N'))
$install = Join-Path $fixture 'installed'
New-Item -ItemType Directory -Path $install | Out-Null
try {
    foreach ($index in 1..70) {
        [IO.File]::WriteAllText((Join-Path $install "fixture-$index.txt"), 'preserve exact fixture bytes')
    }
    $evidence = Join-Path $fixture 'evidence/failure.json'
    Write-UninstallFailureEvidence $install (Join-Path $install 'fixture-1.txt') $evidence
    $result = Get-Content -LiteralPath $evidence -Raw | ConvertFrom-Json
    if (-not $result.entries_truncated -or $result.entries.Count -ne 64) { throw 'Diagnostics did not bound installed entries' }
    if (-not $result.executable_exists) { throw 'Diagnostics lost the original file observation' }
    if ($null -eq $result.restart_manager) { throw 'Restart Manager diagnostics did not compile and execute' }
    if ($result.restart_manager.Owners.Count -gt 64) { throw 'Diagnostics exceeded the owner bound' }
    foreach ($index in 1..70) {
        if ([IO.File]::ReadAllText((Join-Path $install "fixture-$index.txt")) -ne 'preserve exact fixture bytes') {
            throw 'Diagnostics changed fixture contents'
        }
    }
} finally { Remove-Item -LiteralPath $fixture -Recurse -Force }
Write-Output 'Installer failure diagnostics test passed: bounded read-only inventory; no installer or application executed.'
