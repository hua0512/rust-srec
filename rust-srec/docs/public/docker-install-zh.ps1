#Requires -Version 5.1
<#
.SYNOPSIS
    Rust-Srec Windows 安装脚本 (中文版)
.DESCRIPTION
    自动配置 Rust-Srec Docker 部署
.EXAMPLE
    irm https://hua0512.github.io/rust-srec/docker-install-zh.ps1 | iex
.LINK
    https://github.com/hua0512/rust-srec
#>

# Fix encoding for Chinese characters in PowerShell 5.1
if ($PSVersionTable.PSVersion.Major -le 5) {
    $OutputEncoding = [System.Text.Encoding]::UTF8
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
}

[CmdletBinding()]
param(
    [string]$InstallDir = ".\rust-srec",
    [string]$Version = "latest"
)

$ErrorActionPreference = "Stop"
$BaseUrl = "https://hua0512.github.io/rust-srec"

# Colors
function Write-Info { Write-Host "[信息] $args" -ForegroundColor Cyan }
function Write-Success { Write-Host "[完成] $args" -ForegroundColor Green }
function Write-Warn { Write-Host "[警告] $args" -ForegroundColor Yellow }
function Write-Err { Write-Host "[错误] $args" -ForegroundColor Red }

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
    Write-Host "|          Rust-Srec 安装脚本                                |" -ForegroundColor Green
    Write-Host "|          自动直播录制工具                                  |" -ForegroundColor Green
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host ""

    # Check requirements
    Write-Info "检查系统要求..."
    
    if (-not (Test-Docker)) {
        Write-Err "Docker 未安装或未运行。"
        Write-Host ""
        Write-Host "请从以下地址安装 Docker Desktop: https://docs.docker.com/desktop/install/windows-install/"
        exit 1
    }
    
    if (-not (Test-DockerCompose)) {
        Write-Err "Docker Compose 不可用。"
        Write-Host ""
        Write-Host "Docker Compose 应包含在 Docker Desktop 中。"
        exit 1
    }
    
    Write-Success "所有要求已满足"

    # Version selection
    if ($Version -eq "latest") {
        Write-Host ""
        Write-Host "选择安装版本:" -ForegroundColor Yellow
        Write-Host "  1) latest  - 稳定版 (推荐)"
        Write-Host "  2) dev     - 开发版 (最新功能)"
        Write-Host ""
        $versionChoice = Read-Host "请输入选项 [1]"
        switch ($versionChoice) {
            "2" { $Version = "dev" }
            "dev" { $Version = "dev" }
            default { $Version = "latest" }
        }
        Write-Info "已选择版本: $Version"
    }

    # Create installation directory
    Write-Info "创建安装目录: $InstallDir"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Set-Location $InstallDir

    # Create subdirectories
    Write-Info "创建数据目录..."
    @("data", "config", "output", "logs") | ForEach-Object {
        New-Item -ItemType Directory -Path $_ -Force | Out-Null
    }
    Write-Success "目录创建完成"

    # Download configuration files
    Write-Info "下载 docker-compose.yml..."
    Invoke-WebRequest -Uri "$BaseUrl/docker-compose.example.yml" -OutFile "docker-compose.yml" -UseBasicParsing
    Write-Success "docker-compose.yml 下载完成"

    Write-Info "下载 .env 模板..."
    Invoke-WebRequest -Uri "$BaseUrl/env.zh.example" -OutFile ".env" -UseBasicParsing
    Write-Success ".env 下载完成"

    # Generate secure secrets
    Write-Info "生成安全密钥..."
    $jwtSecret = New-SecureSecret -Length 32
    $sessionSecret = New-SecureSecret -Length 32

    # Update .env with generated secrets
    $envContent = Get-Content ".env" -Raw
    $envContent = $envContent -replace "JWT_SECRET=.*", "JWT_SECRET=$jwtSecret"
    $envContent = $envContent -replace "SESSION_SECRET=.*", "SESSION_SECRET=$sessionSecret"
    
    if ($Version -ne "latest") {
        Write-Info "设置版本: $Version"
        $envContent = $envContent -replace "VERSION=.*", "VERSION=$Version"
    }
    
    Set-Content ".env" $envContent -NoNewline -Encoding UTF8
    Write-Success "密钥已生成并配置"

    Write-Host ""
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host "|              安装完成!                                     |" -ForegroundColor Green
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host ""
    Write-Host "安装目录: $(Get-Location)"
    Write-Host ""
    Write-Host "后续步骤:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  1. 查看并自定义配置:"
    Write-Host "     notepad .env" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  2. 启动应用:"
    Write-Host "     docker compose up -d" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  3. 访问 Web 界面:"
    Write-Host "     http://localhost:15275" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  4. 默认登录凭据:"
    Write-Host "     用户名: admin" -ForegroundColor Green
    Write-Host "     密码: admin123!" -ForegroundColor Green
    Write-Host ""
    Write-Host "[!] 重要: 首次登录后请务必修改默认密码!" -ForegroundColor Yellow
    Write-Host ""

    # Ask if user wants to start now
    $response = Read-Host "是否立即启动 Rust-Srec? [y/N]"
    if ($response -match "^[Yy]$") {
        Write-Info "正在启动 Rust-Srec..."
        try {
            docker compose up -d
        } catch {
            docker-compose up -d
        }
        Write-Host ""
        Write-Success "Rust-Srec 已启动!"
        Write-Host ""
        Write-Host "Web 界面: http://localhost:15275" -ForegroundColor Cyan
        Write-Host "API 文档: http://localhost:12555/api/docs" -ForegroundColor Cyan
    } else {
        Write-Info "稍后可使用以下命令启动: docker compose up -d"
    }
}

# Run installation
Install-RustSrec
