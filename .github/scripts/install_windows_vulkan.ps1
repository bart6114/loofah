$ErrorActionPreference = 'Stop'
$version = '1.4.363.0'
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$packages = @{
    X64 = @{ platform = 'windows'; sha256 = '94a82d378f7a5e3e54c9db7d2fb7016af136e14ac0a18dbf0f2f67a36352d141' }
    Arm64 = @{ platform = 'warm'; sha256 = '4af4e6a08eebd55695e7c4bb54a69ed65f21a727b5cb54af4fd78c3489b4b9a0' }
}
if (-not $packages.ContainsKey($architecture) -or -not $env:GITHUB_ENV) {
    throw 'Requires a native Windows x64 or ARM64 GitHub runner'
}
$package = $packages[$architecture]
$sdk = Join-Path $env:RUNNER_TEMP "loofah-vulkan-$version-$architecture"
if (-not (Test-Path "$sdk/Include/vulkan/vulkan.hpp")) {
    $installer = Join-Path $env:RUNNER_TEMP "vulkansdk-$architecture.exe"
    Invoke-WebRequest "https://sdk.lunarg.com/sdk/download/$version/$($package.platform)/vulkansdk-windows-$($architecture.ToUpper())-$version.exe" -OutFile $installer
    if ((Get-FileHash $installer -Algorithm SHA256).Hash -ne $package.sha256) {
        throw 'Vulkan SDK checksum mismatch'
    }
    $setup = Start-Process -FilePath $installer -ArgumentList @('--root', "`"$sdk`"", '--accept-licenses', '--default-answer', '--confirm-command', 'install', 'copy_only=1') -Wait -PassThru
    if ($setup.ExitCode -ne 0) { throw "Vulkan SDK installer exited with $($setup.ExitCode)" }
}
foreach ($file in @('Include/vulkan/vulkan.hpp', 'Bin/glslc.exe', 'Lib/vulkan-1.lib')) {
    if (-not (Test-Path "$sdk/$file")) { throw "Vulkan SDK is missing $file" }
}
"VULKAN_SDK=$sdk" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
"$sdk/Bin" | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
