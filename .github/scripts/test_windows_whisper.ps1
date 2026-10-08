param([switch]$RequireGpu)
$ErrorActionPreference = 'Stop'
$model = Join-Path ([System.IO.Path]::GetTempPath()) 'loofah-whisper-base-q8_0.bin'
$expectedHash = 'c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9'
if (-not (Test-Path $model) -or (Get-FileHash $model -Algorithm SHA256).Hash -ne $expectedHash) {
    Invoke-WebRequest 'https://hyprnote.s3.us-east-1.amazonaws.com/v0/ggerganov/whisper.cpp/main/ggml-base-q8_0.bin' -OutFile $model
}
if ((Get-FileHash $model -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Whisper model checksum mismatch' }
$env:LOOFAH_WHISPER_MODEL = $model
Write-Output "System Vulkan loader present: $(Test-Path "$env:SystemRoot/System32/vulkan-1.dll")"
if ($RequireGpu) {
    Remove-Item Env:LOOFAH_WHISPER_CPU -ErrorAction SilentlyContinue
    Remove-Item Env:GGML_DISABLE_VULKAN -ErrorAction SilentlyContinue
    cargo test --locked --release -p whisper-local --features vulkan --test inference actual_gpu_transcribes -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} else {
    cargo test --locked --release -p whisper-local --features vulkan --test inference automatic_backend_transcribes -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
$env:LOOFAH_WHISPER_CPU = '1'
cargo test --locked --release -p whisper-local --features vulkan --test inference forced_cpu_transcribes_and_cancels -- --ignored --nocapture
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test --locked --release -p transcribe-whisper-local --features vulkan --test inference -- --ignored --nocapture
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
