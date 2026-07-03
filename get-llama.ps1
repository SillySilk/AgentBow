# Downloads the pinned llama.cpp server build into desktop\src-tauri\bin\llama.
# CUDA build when an NVIDIA driver is present, CPU build otherwise. Idempotent.
$ErrorActionPreference = "Stop"
$tag = "b9860"
$dest = Join-Path $PSScriptRoot "desktop\src-tauri\bin\llama"
if (Test-Path (Join-Path $dest "llama-server.exe")) { Write-Host "llama-server present - skipping"; exit 0 }
New-Item -ItemType Directory -Force $dest | Out-Null
$hasNvidia = $null -ne (Get-Command nvidia-smi -ErrorAction SilentlyContinue)
$assets = if ($hasNvidia) {
    @("llama-$tag-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip")
} else {
    @("llama-$tag-bin-win-cpu-x64.zip")
}
foreach ($a in $assets) {
    $url = "https://github.com/ggml-org/llama.cpp/releases/download/$tag/$a"
    $zip = Join-Path $env:TEMP $a
    Write-Host "Downloading $a ..."
    Invoke-WebRequest -Uri $url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $dest -Force
    Remove-Item $zip
}
Write-Host "llama-server ready in $dest"
