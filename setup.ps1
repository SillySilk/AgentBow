# Bow Setup Script
# Run from: C:\AI\agent Bow\
# Usage: powershell -ExecutionPolicy Bypass -File setup.ps1

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host ""
Write-Host "=== Bow Setup ===" -ForegroundColor Cyan

function Check-Command($name) {
    return $null -ne (Get-Command $name -ErrorAction SilentlyContinue)
}

$missing = @()
if (-not (Check-Command "node"))  { $missing += "Node.js 20 LTS  -> https://nodejs.org" }
if (-not (Check-Command "cargo")) { $missing += "Rust            -> https://rustup.rs" }
if (-not (Check-Command "npm"))   { $missing += "npm (comes with Node.js)" }

if ($missing.Count -gt 0) {
    Write-Host ""
    Write-Host "[!] Missing prerequisites:" -ForegroundColor Red
    $missing | ForEach-Object { Write-Host "    - $_" -ForegroundColor Yellow }
    Write-Host ""
    Write-Host "Install them, then re-run this script."
    Write-Host ""
    exit 1
}

Write-Host "[+] Node $(node --version), npm $(npm --version)" -ForegroundColor Green
Write-Host "[+] Rust $(rustc --version)" -ForegroundColor Green

# Generate placeholder icons
Write-Host ""
Write-Host "[*] Generating placeholder icons..." -ForegroundColor Cyan

$iconDir    = Join-Path $Root "desktop\src-tauri\icons"
$extIconDir = Join-Path $Root "extension\icons"
New-Item -ItemType Directory -Force -Path $iconDir    | Out-Null
New-Item -ItemType Directory -Force -Path $extIconDir | Out-Null

Add-Type -AssemblyName System.Drawing

function Create-Icon($path, $size) {
    $bmp   = New-Object System.Drawing.Bitmap($size, $size)
    $g     = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::FromArgb(26, 26, 46))
    $brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(233, 69, 96))
    $fontSize = [Math]::Max(8, $size / 3)
    $font  = New-Object System.Drawing.Font("Arial", $fontSize, [System.Drawing.FontStyle]::Bold)
    $sf    = New-Object System.Drawing.StringFormat
    $sf.Alignment     = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $rect  = New-Object System.Drawing.RectangleF(0, 0, $size, $size)
    $g.DrawString("B", $font, $brush, $rect, $sf)
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

Create-Icon (Join-Path $iconDir "32x32.png")      32
Create-Icon (Join-Path $iconDir "128x128.png")    128
Create-Icon (Join-Path $iconDir "128x128@2x.png") 256
Create-Icon (Join-Path $iconDir "icon.png")       256
Create-Icon (Join-Path $extIconDir "icon16.png")  16
Create-Icon (Join-Path $extIconDir "icon48.png")  48
Create-Icon (Join-Path $extIconDir "icon128.png") 128

Copy-Item (Join-Path $iconDir "32x32.png")   (Join-Path $iconDir "icon.ico")  -Force
Copy-Item (Join-Path $iconDir "128x128.png") (Join-Path $iconDir "icon.icns") -Force

Write-Host "[+] Icons created" -ForegroundColor Green

# Install npm deps - desktop
Write-Host ""
Write-Host "[*] Installing desktop npm deps..." -ForegroundColor Cyan
Push-Location (Join-Path $Root "desktop")
npm install
Pop-Location
Write-Host "[+] Desktop deps installed" -ForegroundColor Green

# Install npm deps - extension
Write-Host ""
Write-Host "[*] Installing extension npm deps..." -ForegroundColor Cyan
Push-Location (Join-Path $Root "extension")
npm install
Pop-Location
Write-Host "[+] Extension deps installed" -ForegroundColor Green

# Check .env for placeholder values
$envFile    = Join-Path $Root "desktop\.env"
$envContent = Get-Content $envFile -Raw
if ($envContent -match "REPLACE_ME") {
    Write-Host ""
    Write-Host "[!] IMPORTANT: Edit desktop\.env and fill in your real keys:" -ForegroundColor Yellow
    Write-Host "    ANTHROPIC_API_KEY=sk-ant-..." -ForegroundColor White
    Write-Host "    TAVILY_API_KEY=tvly-..."      -ForegroundColor White
    Write-Host "    BOW_SECRET=<32+ random chars>" -ForegroundColor White
    Write-Host ""
    Write-Host "    Generate BOW_SECRET:" -ForegroundColor Gray
    Write-Host '    -join ((65..90)+(97..122)+(48..57) | Get-Random -Count 32 | ForEach-Object {[char]$_})' -ForegroundColor Gray
}

# Build extension
Write-Host ""
Write-Host "[*] Building Chrome extension..." -ForegroundColor Cyan
Push-Location (Join-Path $Root "extension")
npm run build
Pop-Location
Write-Host "[+] Extension built -> extension\dist\" -ForegroundColor Green

# Add Rust target
Write-Host ""
Write-Host "[*] Adding Rust Windows target..." -ForegroundColor Cyan
rustup target add x86_64-pc-windows-msvc
Write-Host "[+] Rust target OK" -ForegroundColor Green

Write-Host ""
Write-Host "=== Setup Complete ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Next steps:" -ForegroundColor White
Write-Host "  1. Edit desktop\.env with your real API keys"
Write-Host "  2. cd desktop && npm run tauri dev"
Write-Host "  3. Load extension\dist as unpacked in chrome://extensions"
Write-Host "  4. Open sidebar -> Settings -> paste BOW_SECRET"
Write-Host ""
