#Requires -Version 5.1
<#
.SYNOPSIS
    Rust-Srec Installation Script for Windows
.DESCRIPTION
    Automatically sets up Rust-Srec Docker deployment
.EXAMPLE
    irm https://hua0512.github.io/rust-srec/docker-install.ps1 | iex
.LINK
    https://github.com/hua0512/rust-srec
#>

[CmdletBinding()]
param(
    [string]$InstallDir = ".\rust-srec",
    [string]$Version = "latest"
)

$ErrorActionPreference = "Stop"
$BaseUrl = "https://hua0512.github.io/rust-srec"

# Colors
function Write-Info { Write-Host "[INFO] $args" -ForegroundColor Cyan }
function Write-Success { Write-Host "[OK] $args" -ForegroundColor Green }
function Write-Warn { Write-Host "[WARN] $args" -ForegroundColor Yellow }
function Write-Err { Write-Host "[ERROR] $args" -ForegroundColor Red }

# Generate secure random hex string
function New-SecureSecret {
    param([int]$Length = 32)
    $bytes = New-Object Byte[] $Length
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    return -join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
}

# Check for Docker
function Test-Docker {
    try {
        $null = docker --version 2>&1
        return $true
    } catch {
        return $false
    }
}

# Check for Docker Compose
function Test-DockerCompose {
    try {
        $null = docker compose version 2>&1
        return $true
    } catch {
        try {
            $null = docker-compose --version 2>&1
            return $true
        } catch {
            return $false
        }
    }
}

# Main installation
function Install-RustSrec {
    Write-Host ""
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host "|          Rust-Srec Installation Script                     |" -ForegroundColor Green
    Write-Host "|          Automatic Streaming Recorder                      |" -ForegroundColor Green
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host ""

    # Check requirements
    Write-Info "Checking requirements..."
    
    if (-not (Test-Docker)) {
        Write-Err "Docker is not installed or not running."
        Write-Host ""
        Write-Host "Please install Docker Desktop from: https://docs.docker.com/desktop/install/windows-install/"
        exit 1
    }
    
    if (-not (Test-DockerCompose)) {
        Write-Err "Docker Compose is not available."
        Write-Host ""
        Write-Host "Docker Compose should be included with Docker Desktop."
        exit 1
    }
    
    Write-Success "All requirements met"

    # Version selection
    if ($Version -eq "latest") {
        Write-Host ""
        Write-Host "Select version to install:" -ForegroundColor Yellow
        Write-Host "  1) latest  - Stable release (recommended)"
        Write-Host "  2) dev     - Development build (bleeding edge)"
        Write-Host ""
        $versionChoice = Read-Host "Enter choice [1]"
        switch ($versionChoice) {
            "2" { $Version = "dev" }
            "dev" { $Version = "dev" }
            default { $Version = "latest" }
        }
        Write-Info "Selected version: $Version"
    }

    # Create installation directory
    Write-Info "Creating installation directory: $InstallDir"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Set-Location $InstallDir

    # Create subdirectories
    Write-Info "Creating data directories..."
    @("data", "config", "output", "logs") | ForEach-Object {
        New-Item -ItemType Directory -Path $_ -Force | Out-Null
    }
    Write-Success "Directories created"

    # Download configuration files
    Write-Info "Downloading docker-compose.yml..."
    Invoke-WebRequest -Uri "$BaseUrl/docker-compose.example.yml" -OutFile "docker-compose.yml" -UseBasicParsing
    Write-Success "docker-compose.yml downloaded"

    Write-Info "Downloading .env template..."
    Invoke-WebRequest -Uri "$BaseUrl/env.example" -OutFile ".env" -UseBasicParsing
    Write-Success ".env downloaded"

    # Generate secure secrets
    Write-Info "Generating secure secrets..."
    $jwtSecret = New-SecureSecret -Length 32
    $sessionSecret = New-SecureSecret -Length 32

    # Update .env with generated secrets
    $envContent = Get-Content ".env" -Raw
    $envContent = $envContent -replace "JWT_SECRET=.*", "JWT_SECRET=$jwtSecret"
    $envContent = $envContent -replace "SESSION_SECRET=.*", "SESSION_SECRET=$sessionSecret"
    
    if ($Version -ne "latest") {
        Write-Info "Setting version to: $Version"
        $envContent = $envContent -replace "VERSION=.*", "VERSION=$Version"
    }
    
    Set-Content ".env" $envContent -NoNewline -Encoding UTF8
    Write-Success "Secrets generated and configured"

    Write-Host ""
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host "|              Installation Complete!                        |" -ForegroundColor Green
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host ""
    Write-Host "Installation directory: $(Get-Location)"
    Write-Host ""
    Write-Host "Next steps:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  1. Review and customize your configuration:"
    Write-Host "     notepad .env" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  2. Start the application:"
    Write-Host "     docker compose up -d" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  3. Access the web interface:"
    Write-Host "     http://localhost:15275" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  4. Default login credentials:"
    Write-Host "     Username: admin" -ForegroundColor Green
    Write-Host "     Password: admin123!" -ForegroundColor Green
    Write-Host ""
    Write-Host "[!] Important: Change the default password after first login!" -ForegroundColor Yellow
    Write-Host ""

    # Ask if user wants to start now
    $response = Read-Host "Would you like to start Rust-Srec now? [y/N]"
    if ($response -match "^[Yy]$") {
        Write-Info "Starting Rust-Srec..."
        try {
            docker compose up -d
        } catch {
            docker-compose up -d
        }
        Write-Host ""
        Write-Success "Rust-Srec is now running!"
        Write-Host ""
        Write-Host "Web Interface: http://localhost:15275" -ForegroundColor Cyan
        Write-Host "API Docs:      http://localhost:12555/api/docs" -ForegroundColor Cyan
    } else {
        Write-Info "You can start Rust-Srec later with: docker compose up -d"
    }
}

# Run installation
Install-RustSrec
