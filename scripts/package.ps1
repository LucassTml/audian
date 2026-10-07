# Builds a publishable installer: dist\Audian-Setup-<version>-x64.exe
#
# 1. builds the app binaries (release)
# 2. builds the installer, which embeds them (compressed)
# 3. copies it to .\dist and prints its size and SHA-256 (for release notes / winget)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot "build.ps1")

Push-Location $root
try {
    $env:AUDIAN_PAYLOAD_DIR = Join-Path $root "target\release"
    cargo build --release -p audian-setup
    if ($LASTEXITCODE -ne 0) { throw "installer build failed" }
    $version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $out = Join-Path $root "dist"
    New-Item -ItemType Directory -Force $out | Out-Null
    $dest = Join-Path $out "Audian-Setup-$version-x64.exe"
    Copy-Item (Join-Path $root "target\release\audian-setup.exe") $dest -Force
    $hash = (Get-FileHash $dest -Algorithm SHA256).Hash
    $size = [math]::Round((Get-Item $dest).Length / 1MB, 1)
    Write-Host ""
    Write-Host "Installer: $dest"
    Write-Host "Size:      $size MB"
    Write-Host "SHA-256:   $hash"
} finally {
    Remove-Item Env:\AUDIAN_PAYLOAD_DIR -ErrorAction SilentlyContinue
    Pop-Location
}
