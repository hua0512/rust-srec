# Rust-Srec Install Loader
# This is an ASCII-only bootstrap that downloads and executes the full installer with proper UTF-8 encoding
#
# Usage:
#   irm https://docs.srec.rs/install.ps1 | iex
#
# For Chinese version:
#   $env:SREC_LANG = "zh"; irm https://docs.srec.rs/install.ps1 | iex
#
# With custom parameters:
#   $env:RUST_SREC_DIR = "C:\my-path"; $env:VERSION = "dev"; irm https://docs.srec.rs/install.ps1 | iex

$ErrorActionPreference = "Stop"
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12

$baseUrl = "https://docs.srec.rs"

# Detect language: explicit env var > system locale > default to English
$useChinese = $false
if ($env:SREC_LANG -eq "zh") {
    $useChinese = $true
} elseif (-not $env:SREC_LANG) {
    # Auto-detect from system locale
    $culture = [System.Globalization.CultureInfo]::CurrentCulture.Name
    if ($culture -match "^zh") {
        $useChinese = $true
    }
}

$scriptName = if ($useChinese) { "docker-install-zh.ps1" } else { "docker-install.ps1" }
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
