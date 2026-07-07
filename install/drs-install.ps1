# =============================================================================
# drs · no-Rust installer for Windows (download a prebuilt `drs.exe`)
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.ps1 | iex
#
# Env overrides:
#   $env:DRS_VERSION     = "v0.3.2"           # specific tag (default: latest)
#   $env:DRS_INSTALL_DIR = "$HOME\bin"        # install dir (default: %LOCALAPPDATA%\drs\bin)
#   $env:DRS_REPO        = "owner/name"       # source repo (default: MageGojo/drission-rs)
# =============================================================================
$ErrorActionPreference = "Stop"

$Repo    = if ($env:DRS_REPO) { $env:DRS_REPO } else { "MageGojo/drission-rs" }
$Version = if ($env:DRS_VERSION) { $env:DRS_VERSION } else { "latest" }
$InstallDir = if ($env:DRS_INSTALL_DIR) { $env:DRS_INSTALL_DIR } else { "$env:LOCALAPPDATA\drs\bin" }

$arch = $env:PROCESSOR_ARCHITECTURE
switch ($arch) {
    "AMD64" { $rarch = "x86_64" }
    "ARM64" { $rarch = "aarch64" }
    default { throw "unsupported architecture: $arch" }
}
$target = "$rarch-pc-windows-msvc"
$asset  = "drs-$target.zip"
Write-Host "==> platform: windows/$arch -> $target" -ForegroundColor Cyan

if ($Version -eq "latest") {
    $url = "https://github.com/$Repo/releases/latest/download/$asset"
} else {
    $url = "https://github.com/$Repo/releases/download/$Version/$asset"
}

$tmp = Join-Path $env:TEMP ("drs-install-" + [System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$zip = Join-Path $tmp $asset

Write-Host "==> downloading $asset ($Version)" -ForegroundColor Cyan
try {
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
} catch {
    throw "download failed from $url`nCheck that release asset '$asset' exists, or build from source: cargo install --path crates/drission-cli --bin drs"
}

Write-Host "==> extracting" -ForegroundColor Cyan
Expand-Archive -Path $zip -DestinationPath $tmp -Force
$bin = Get-ChildItem -Path $tmp -Recurse -Filter "drs.exe" | Select-Object -First 1
if (-not $bin) { throw "no drs.exe found inside $asset" }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $bin.FullName -Destination (Join-Path $InstallDir "drs.exe") -Force
Remove-Item -Recurse -Force $tmp
Write-Host "OK installed: $InstallDir\drs.exe" -ForegroundColor Green

# add to user PATH if missing
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Host "! Added $InstallDir to your user PATH. Open a new terminal to use 'drs'." -ForegroundColor Yellow
}

& "$InstallDir\drs.exe" --version
Write-Host ""
Write-Host "Next: configure the MCP server for Cursor / Codex:" -ForegroundColor Green
Write-Host "    drs setup"
Write-Host ""
Write-Host "Then ask your AI agent to use the drs browser tools for any hard-to-get web data."
