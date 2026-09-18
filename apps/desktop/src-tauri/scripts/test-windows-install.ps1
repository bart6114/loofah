$ErrorActionPreference = 'Stop'
$tauri = Split-Path $PSScriptRoot -Parent
$config = Get-Content (Join-Path $tauri 'tauri.conf.staging.json') -Raw | ConvertFrom-Json
$installers = @(Get-ChildItem (Join-Path $tauri 'target/release/bundle/nsis/*-setup.exe'))
if ($installers.Count -ne 1) { throw 'Expected one preview installer' }
$install = Join-Path $env:RUNNER_TEMP 'Loofah Install Test'

function Run-Installer($file, $arguments) {
    $process = Start-Process -FilePath $file -ArgumentList $arguments -PassThru
    if (-not $process.WaitForExit(180000)) {
        Stop-Process -Id $process.Id -Force
        throw 'Installer timed out'
    }
    if ($process.ExitCode -ne 0) { throw "Installer exited with $($process.ExitCode)" }
}

Run-Installer $installers[0].FullName "/S /D=$install"
$executable = Join-Path $install "$($config.mainBinaryName).exe"
if (-not (Test-Path $executable)) { throw 'Installed desktop executable missing' }
if (-not (Test-Path (Join-Path $install 'vcruntime140.dll'))) { throw 'Installed CRT missing' }
& (Join-Path $install 'loof.exe') --version
if ($LASTEXITCODE -ne 0) { throw 'Installed CLI failed to start' }
& (Join-Path $install 'loof.exe') mcp --help
if ($LASTEXITCODE -ne 0) { throw 'Installed MCP command failed to start' }

$app = Start-Process -FilePath $executable -PassThru
try {
    if ($app.WaitForExit(10000)) { throw "Installed app exited during startup: $($app.ExitCode)" }
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        $app.Refresh()
        if ($app.HasExited) { throw "Installed app exited before showing a window: $($app.ExitCode)" }
        if ($app.MainWindowHandle -ne 0) { break }
        Start-Sleep -Milliseconds 500
    }
    if ($app.MainWindowHandle -eq 0) { throw 'Installed app did not show a desktop window within 30 seconds' }
} finally {
    if (-not $app.HasExited) {
        Stop-Process -Id $app.Id -Force
        $app.WaitForExit()
    }
}

$sentinel = Join-Path $install 'personal-file.txt'
Set-Content $sentinel 'preserve unknown files'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (-not (Test-Path $runKey)) { New-Item -Path $runKey -Force | Out-Null }
New-ItemProperty -Path $runKey -Name $config.productName -Value "`"$executable`" --background" -PropertyType String -Force | Out-Null
$cliBin = Join-Path $env:LOCALAPPDATA 'Loofah/cli/loof-staging/bin'
New-Item -ItemType Directory -Path $cliBin -Force | Out-Null
$managedCli = Join-Path $cliBin 'loof-staging.exe'
Copy-Item (Join-Path $install 'loof.exe') $managedCli
$hash = (Get-FileHash $managedCli -Algorithm SHA256).Hash.ToLowerInvariant()
@{ files = @{ 'loof-staging.exe' = $hash } } | ConvertTo-Json | Set-Content (Join-Path $cliBin '.loofah-cli.json') -Encoding utf8NoBOM
$originalPath = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', "$originalPath;$cliBin", 'User')
$uninstaller = Join-Path $env:RUNNER_TEMP 'loofah-uninstall-test.exe'
Copy-Item (Join-Path $install 'uninstall.exe') $uninstaller
Run-Installer $uninstaller "/S _?=$install"
if (Test-Path $executable) { throw 'Uninstall left the desktop executable behind' }
if (-not (Test-Path $sentinel)) { throw 'Uninstall removed an unmanaged file' }
if (Test-Path $cliBin) { throw 'Uninstall left the managed CLI behind' }
if ([Environment]::GetEnvironmentVariable('Path', 'User').Split(';') -contains $cliBin) { throw 'Uninstall left the managed CLI on PATH' }
if (Get-ItemProperty -Path $runKey -Name $config.productName -ErrorAction SilentlyContinue) { throw 'Uninstall left the startup entry behind' }
Write-Output 'Installer, app startup, installed CLI/MCP, and uninstall smoke tests passed.'
