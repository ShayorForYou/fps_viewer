# Packages an already-built FpsViewer.exe on Windows (used by CI):
#   dist/FpsViewer-x64.exe, dist/FpsViewer-Setup-x64.exe, dist/SHA256SUMS.txt
#
#   pwsh installer/package.ps1 [-Exe target/x86_64-pc-windows-msvc/release/FpsViewer.exe]
#
# The version comes from Cargo.toml. Needs NSIS (makensis) — preinstalled on GitHub's
# Windows runners, otherwise `choco install nsis` or `winget install NSIS.NSIS`.
param(
    [string]$Exe = "target/x86_64-pc-windows-msvc/release/FpsViewer.exe"
)
$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "FPS Viewer $version"

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) {
    foreach ($p in @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe")) {
        if (Test-Path $p) { $makensis = $p; break }
    }
}
if (-not $makensis) { throw "makensis not found. Install NSIS (choco install nsis)." }

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue "$dist\FpsViewer-x64.exe", "$dist\FpsViewer-Setup-x64.exe", "$dist\SHA256SUMS.txt"

Copy-Item $Exe "$dist\FpsViewer-x64.exe"

& $makensis /V2 "/DVERSION=$version" "/DICON=$root\assets\app.ico" "/DEXE=$dist\FpsViewer-x64.exe" `
    "/DOUTFILE=$dist\FpsViewer-Setup-x64.exe" "$root\installer\FpsViewer.nsi"
if ($LASTEXITCODE -ne 0) { throw "makensis failed ($LASTEXITCODE)" }

Get-FileHash "$dist\FpsViewer-x64.exe", "$dist\FpsViewer-Setup-x64.exe" -Algorithm SHA256 |
    ForEach-Object { "$($_.Hash.ToLower())  $(Split-Path $_.Path -Leaf)" } |
    Set-Content -Encoding utf8NoBOM "$dist\SHA256SUMS.txt"

Get-ChildItem $dist | Format-Table Name, Length
Get-Content "$dist\SHA256SUMS.txt"
if ($env:GITHUB_OUTPUT) { "version=$version" | Out-File -Append -Encoding utf8 $env:GITHUB_OUTPUT }
