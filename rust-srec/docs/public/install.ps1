# Rust-Srec Install Loader
# This is an ASCII-only bootstrap that downloads and executes the full installer with proper UTF-8 encoding
# Usage: irm https://docs.srec.rs/install.ps1 | iex
# For Chinese: irm https://docs.srec.rs/install.ps1 | iex; Install-RustSrec -Chinese

param([switch]$Chinese)

$ErrorActionPreference = "Stop"
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12

$baseUrl = "https://docs.srec.rs"
$scriptName = if ($Chinese -or $env:LANG -match "zh") { "docker-install-zh.ps1" } else { "docker-install.ps1" }
$scriptUrl = "$baseUrl/$scriptName"

try {
    $webClient = New-Object System.Net.WebClient
    $webClient.Encoding = [System.Text.Encoding]::UTF8
    $scriptContent = $webClient.DownloadString($scriptUrl)
    $scriptBlock = [scriptblock]::Create($scriptContent)
    & $scriptBlock
} catch {
    Write-Host "[ERROR] Failed to download or execute installer: $_" -ForegroundColor Red
    Write-Host ""
    Write-Host "Try downloading manually from: $scriptUrl" -ForegroundColor Yellow
}
