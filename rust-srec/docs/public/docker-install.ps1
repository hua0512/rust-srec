#
# Rust-Srec Installation Script for Windows
# https://github.com/hua0512/rust-srec
#
# Usage (via bootstrap loader):
#   irm https://docs.srec.rs/install.ps1 | iex
#
# With parameters:
#   $env:RUST_SREC_DIR = "C:\my-path"; $env:VERSION = "dev"; irm https://docs.srec.rs/install.ps1 | iex
#

#Requires -Version 5.1

# Fix encoding for PowerShell 5.1
if ($PSVersionTable.PSVersion.Major -le 5) {
    $OutputEncoding = [System.Text.Encoding]::UTF8
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
}

$ErrorActionPreference = "Stop"
$script:BaseUrl = "https://docs.srec.rs"
$script:InstallDir = if ($env:RUST_SREC_DIR) { $env:RUST_SREC_DIR } else { ".\rust-srec" }
$script:Version = if ($env:VERSION) { $env:VERSION } else { "latest" }

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
        return $LASTEXITCODE -eq 0
    } catch {
        return $false
    }
}

# Check for Docker Compose
function Test-DockerCompose {
    try {
        $null = docker compose version 2>&1
        if ($LASTEXITCODE -eq 0) { return $true }
    } catch {}
    try {
        $null = docker-compose --version 2>&1
        return $LASTEXITCODE -eq 0
    } catch {
        return $false
    }
}

# Download file with proper encoding handling
function Get-RemoteFile {
    param(
        [string]$Url,
        [string]$OutFile
    )
    try {
        # Use Invoke-WebRequest and read raw bytes from stream
        $response = Invoke-WebRequest -Uri $Url -UseBasicParsing
        # Use RawContentStream to get bytes (works for both text and binary)
        $stream = $response.RawContentStream
        $stream.Position = 0
        $bytes = New-Object byte[] $stream.Length
        $null = $stream.Read($bytes, 0, $stream.Length)
        [System.IO.File]::WriteAllBytes($OutFile, $bytes)
    } catch {
        Write-Err "Failed to download: $Url"
        throw
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
        return
    }
    
    if (-not (Test-DockerCompose)) {
        Write-Err "Docker Compose is not available."
        Write-Host ""
        Write-Host "Docker Compose should be included with Docker Desktop."
        return
    }
    
    Write-Success "All requirements met"

    # Version selection
    if ($script:Version -eq "latest") {
        Write-Host ""
        Write-Host "Select version to install:" -ForegroundColor Yellow
        Write-Host "  1) latest  - Stable release (recommended)"
        Write-Host "  2) dev     - Development build (bleeding edge)"
        Write-Host ""
        $versionChoice = Read-Host "Enter choice [1]"
        switch ($versionChoice) {
            "2" { $script:Version = "dev" }
            "dev" { $script:Version = "dev" }
            default { $script:Version = "latest" }
        }
    }
    Write-Info "Selected version: $($script:Version)"

    # Create installation directory
    Write-Info "Creating installation directory: $($script:InstallDir)"
    New-Item -ItemType Directory -Path $script:InstallDir -Force | Out-Null
    Push-Location $script:InstallDir

    try {
        # Download configuration files
        Write-Info "Downloading docker-compose.yml..."
        Get-RemoteFile -Url "$($script:BaseUrl)/docker-compose.example.yml" -OutFile "docker-compose.yml"
        Write-Success "docker-compose.yml downloaded"

        Write-Info "Downloading .env template..."
        Get-RemoteFile -Url "$($script:BaseUrl)/env.example" -OutFile ".env"
        Write-Success ".env downloaded"

        # Generate secure secrets
        Write-Info "Generating secure secrets..."
        $jwtSecret = New-SecureSecret -Length 32
        $sessionSecret = New-SecureSecret -Length 32

        # Update .env with generated secrets
        $envContent = Get-Content ".env" -Raw -Encoding UTF8
        $envContent = $envContent -replace "JWT_SECRET=.*", "JWT_SECRET=$jwtSecret"
        $envContent = $envContent -replace "SESSION_SECRET=.*", "SESSION_SECRET=$sessionSecret"
        
        if ($script:Version -ne "latest") {
            Write-Info "Setting version to: $($script:Version)"
            $envContent = $envContent -replace "VERSION=.*", "VERSION=$($script:Version)"
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
    } finally {
        Pop-Location
    }
}

# Run installation
Install-RustSrec
