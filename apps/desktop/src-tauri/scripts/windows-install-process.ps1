function Complete-OwnedInstallProcess {
    param($Process, [switch]$Stop, [switch]$RequireZeroExit, $PrimaryError = $null, [int]$TimeoutMs = 180000)

    $failure = $PrimaryError
    $joined = $false
    $killAttempted = $false
    try {
        if ($Stop -and -not $Process.HasExited) {
            $killAttempted = $true
            $Process.Kill()
        }
        $joined = $Process.WaitForExit($TimeoutMs)
        if (-not $joined) { throw 'Owned installer test process timed out' }
        if ($RequireZeroExit -and $Process.ExitCode -ne 0) {
            throw "Installer exited with $($Process.ExitCode)"
        }
    } catch {
        if ($null -eq $failure) { $failure = $_ }
        else { Write-Warning "Owned process cleanup failed: $($_.Exception.Message)" }
    } finally {
        if (-not $joined) {
            try {
                if (-not $Process.HasExited -and -not $killAttempted) { $Process.Kill() }
                if (-not $Process.WaitForExit(30000)) { throw 'Owned process did not exit after termination' }
            } catch {
                if ($null -eq $failure) { $failure = $_ }
                else { Write-Warning "Owned process termination failed: $($_.Exception.Message)" }
            }
        }
        try { $Process.Dispose() }
        catch {
            if ($null -eq $failure) { $failure = $_ }
            else { Write-Warning "Owned process disposal failed: $($_.Exception.Message)" }
        }
    }
    if ($null -ne $failure) { throw $failure }
}
