#
# Rust-Srec 安装脚本 (中文版)
# https://github.com/hua0512/rust-srec
#
# 使用方法 (通过引导加载器):
#   irm https://docs.srec.rs/install.ps1 | iex
#
# 使用参数:
#   $env:RUST_SREC_DIR = "C:\my-path"; $env:VERSION = "dev"; irm https://docs.srec.rs/install.ps1 | iex
#

#Requires -Version 5.1

# Fix encoding for Chinese characters in PowerShell 5.1
if ($PSVersionTable.PSVersion.Major -le 5) {
    $OutputEncoding = [System.Text.Encoding]::UTF8
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
}

$ErrorActionPreference = "Stop"
$script:BaseUrl = "https://docs.srec.rs"
$script:InstallDir = if ($env:RUST_SREC_DIR) { $env:RUST_SREC_DIR } else { ".\rust-srec" }
$script:Version = if ($env:VERSION) { $env:VERSION } else { "latest" }

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
        # Use -OutFile which respects PowerShell's current location (Push-Location)
        # Note: [System.IO.File]::WriteAllBytes() would use .NET's working directory instead
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    } catch {
        Write-Err "下载失败: $Url"
        throw
    }
}

# Main installation
function Install-RustSrec {
    Write-Host ""
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host "|                Rust-Srec 安装脚本                          |" -ForegroundColor Green
    Write-Host "|                自动直播录制工具                            |" -ForegroundColor Green
    Write-Host "+============================================================+" -ForegroundColor Green
    Write-Host ""

    # Check requirements
    Write-Info "检查系统要求..."
    
    if (-not (Test-Docker)) {
        Write-Err "Docker 未安装或未运行。"
        Write-Host ""
        Write-Host "请从以下地址安装 Docker Desktop: https://docs.docker.com/desktop/install/windows-install/"
        return
    }
    
    if (-not (Test-DockerCompose)) {
        Write-Err "Docker Compose 不可用。"
        Write-Host ""
        Write-Host "Docker Compose 应包含在 Docker Desktop 中。"
        return
    }
    
    Write-Success "所有要求已满足"

    # Version selection
    if ($script:Version -eq "latest") {
        Write-Host ""
        Write-Host "选择安装版本:" -ForegroundColor Yellow
        Write-Host "  1) latest  - 稳定版 (推荐)"
        Write-Host "  2) dev     - 开发版 (最新功能)"
        Write-Host ""
        $versionChoice = Read-Host "请输入选项 [1]"
        switch ($versionChoice) {
            "2" { $script:Version = "dev" }
            "dev" { $script:Version = "dev" }
            default { $script:Version = "latest" }
        }
    }
    Write-Info "已选择版本: $($script:Version)"

    # Create installation directory
    Write-Info "创建安装目录: $($script:InstallDir)"
    New-Item -ItemType Directory -Path $script:InstallDir -Force | Out-Null
    Push-Location $script:InstallDir

    try {
        # Download configuration files
        Write-Info "下载 docker-compose.yml..."
        Get-RemoteFile -Url "$($script:BaseUrl)/docker-compose.example.yml" -OutFile "docker-compose.yml"
        Write-Success "docker-compose.yml 下载完成"

        Write-Info "下载 .env 模板..."
        Get-RemoteFile -Url "$($script:BaseUrl)/env.zh.example" -OutFile ".env"
        Write-Success ".env 下载完成"

        # Generate secure secrets
        Write-Info "生成安全密钥..."
        $jwtSecret = New-SecureSecret -Length 32
        $sessionSecret = New-SecureSecret -Length 32

        # Update .env with generated secrets
        $envContent = Get-Content ".env" -Raw -Encoding UTF8
        $envContent = $envContent -replace "JWT_SECRET=.*", "JWT_SECRET=$jwtSecret"
        $envContent = $envContent -replace "SESSION_SECRET=.*", "SESSION_SECRET=$sessionSecret"
        
        if ($script:Version -ne "latest") {
            Write-Info "设置版本: $($script:Version)"
            $envContent = $envContent -replace "VERSION=.*", "VERSION=$($script:Version)"
        }
        
        Set-Content ".env" $envContent -NoNewline -Encoding UTF8
        Write-Success "密钥已生成并配置"

        Write-Host ""
        Write-Host "+============================================================+" -ForegroundColor Green
        Write-Host "|                      安装完成!                             |" -ForegroundColor Green
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
            $startSuccess = $false
            
            # Try docker compose first (Docker Desktop v2+)
            docker compose up -d 2>&1 | Out-Host
            if ($LASTEXITCODE -eq 0) {
                $startSuccess = $true
            } else {
                # Fallback to docker-compose (standalone)
                docker-compose up -d 2>&1 | Out-Host
                if ($LASTEXITCODE -eq 0) {
                    $startSuccess = $true
                }
            }
            
            Write-Host ""
            if ($startSuccess) {
                Write-Success "Rust-Srec 已启动!"
                Write-Host ""
                Write-Host "Web 界面: http://localhost:15275" -ForegroundColor Cyan
                Write-Host "API 文档: http://localhost:12555/api/docs" -ForegroundColor Cyan
            } else {
                Write-Err "启动 Rust-Srec 失败。请检查 Docker 是否正在运行，然后重试。"
                Write-Host ""
                Write-Host "手动启动命令: docker compose up -d" -ForegroundColor Yellow
            }
        } else {
            Write-Info "稍后可使用以下命令启动: docker compose up -d"
        }
    } finally {
        Pop-Location
    }
}

# Run installation
Install-RustSrec
