# Mike aur Donna — one-command installer (Windows / PowerShell).
#
# Installs prerequisites, downloads pdfium, builds the desktop app, and
# produces an installer you run once. After that, search "Mike" in Start.
#
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
#
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root
Write-Host "==> Mike aur Donna installer"
Write-Host "    repo: $Root"

# 1. Rust
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Write-Host "==> Installing Rust…"
  Invoke-WebRequest "https://win.rustup.rs/x86_64" -OutFile "$env:TEMP\rustup-init.exe"
  & "$env:TEMP\rustup-init.exe" -y
  $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
}

# 2. Node.js (needed to build the frontend)
if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
  throw "Node.js (v18+) is required. Install it from https://nodejs.org/ and re-run."
}

# 3. Tauri CLI
cargo tauri --version 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) {
  Write-Host "==> Installing Tauri CLI…"
  cargo install tauri-cli --version "^2" --locked
}

# 4. pdfium (bundled into the app)
$PdfiumDir = Join-Path $Root "src-tauri\pdfium"
New-Item -ItemType Directory -Force -Path $PdfiumDir | Out-Null
$Lib = Join-Path $PdfiumDir "pdfium.dll"
if (-not (Test-Path $Lib)) {
  Write-Host "==> Downloading pdfium…"
  $arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "x64" }
  $Tmp = Join-Path $env:TEMP "mike_pdfium"
  New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
  Invoke-WebRequest "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-win-$arch.zip" -OutFile "$Tmp\pdfium.zip"
  Expand-Archive "$Tmp\pdfium.zip" -DestinationPath $Tmp -Force
  Copy-Item "$Tmp\bin\pdfium.dll" $Lib -Force
  Remove-Item -Recurse -Force $Tmp
}

# 5. Frontend dependencies
Write-Host "==> Installing frontend dependencies…"
npm --prefix ./frontend install

# 6. Build the desktop app
Write-Host "==> Building the desktop app (first build takes a few minutes)…"
cargo tauri build

# 7. Done
Write-Host ""
Write-Host "==> Build complete. Your installer (.msi / .exe) is under:"
Write-Host "    target\release\bundle\  (or src-tauri\target\release\bundle\)"
Write-Host "    Run it to install Mike aur Donna, then search 'Mike' in the Start menu."
Write-Host ""
Write-Host "    Note: if the OCR build step fails, install Tesseract for Windows and"
Write-Host "    ensure its headers/libs are discoverable (e.g. via vcpkg)."
