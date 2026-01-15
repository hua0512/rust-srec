#!/usr/bin/env bash
#
# Rust-Srec 安装脚本 (中文版)
# https://github.com/hua0512/rust-srec
#
# 使用方法:
#   curl -fsSL https://docs.srec.rs/docker-install-zh.sh | bash
#   wget -qO- https://docs.srec.rs/docker-install-zh.sh | bash
#

set -euo pipefail

# Colors for output
RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[1;33m'
BLUE=$'\033[0;34m'
NC=$'\033[0m' # No Color

# Default values
INSTALL_DIR="${RUST_SREC_DIR:-./rust-srec}"
VERSION="${VERSION:-latest}"
BASE_URL="https://docs.srec.rs"

# Print colored messages
info() { echo -e "${BLUE}[信息]${NC} $*"; }
success() { echo -e "${GREEN}[完成]${NC} $*"; }
warn() { echo -e "${YELLOW}[警告]${NC} $*"; }
error() { echo -e "${RED}[错误]${NC} $*" >&2; }

# Generate secure random string
generate_secret() {
    local length="${1:-32}"
    if command -v openssl &>/dev/null; then
        openssl rand -hex "$length"
    elif [ -r /dev/urandom ]; then
        head -c "$length" /dev/urandom | od -An -tx1 | tr -d ' \n'
    else
        local secret=""
        for _ in $(seq 1 "$length"); do
            secret+=$(printf '%x' $((RANDOM % 16)))
        done
        echo "$secret"
    fi
}

# Check for required commands
check_requirements() {
    local missing=()
    
    if ! command -v docker &>/dev/null; then
        missing+=("docker")
    fi
    
    if ! command -v docker-compose &>/dev/null && ! docker compose version &>/dev/null 2>&1; then
        missing+=("docker-compose")
    fi
    
    if ! command -v curl &>/dev/null && ! command -v wget &>/dev/null; then
        missing+=("curl 或 wget")
    fi
    
    if [ ${#missing[@]} -gt 0 ]; then
        error "缺少必要工具: ${missing[*]}"
        echo ""
        echo "请先安装以下工具后再运行此脚本:"
        for tool in "${missing[@]}"; do
            echo "  - $tool"
        done
        exit 1
    fi
}

# Download file using curl or wget
download() {
    local url="$1"
    local output="$2"
    
    if command -v curl &>/dev/null; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget &>/dev/null; then
        wget -qO "$output" "$url"
    else
        error "未找到 curl 或 wget"
        exit 1
    fi
}

# Main installation
main() {
    echo ""
    echo -e "${GREEN}+============================================================+${NC}"
    echo -e "${GREEN}|${NC}                ${BLUE}Rust-Srec 安装脚本${NC}                          ${GREEN}|${NC}"
    echo -e "${GREEN}|${NC}                自动直播录制工具                             ${GREEN}|${NC}"
    echo -e "${GREEN}+============================================================+${NC}"
    echo ""
    
    # Check requirements
    info "检查系统要求..."
    check_requirements
    success "所有要求已满足"
    
    # Version selection
    if [ "$VERSION" = "latest" ]; then
        echo ""
        echo -e "${YELLOW}选择安装版本:${NC}"
        echo "  1) latest  - 稳定版 (推荐)"
        echo "  2) dev     - 开发版 (最新功能)"
        echo ""
        read -p "请输入选项 [1]: " version_choice < /dev/tty
        case "$version_choice" in
            2|dev)
                VERSION="dev"
                ;;
            *)
                VERSION="latest"
                ;;
        esac
        info "已选择版本: $VERSION"
    fi
    
    # Create installation directory
    info "创建安装目录: $INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
    cd "$INSTALL_DIR"
    
    # Download configuration files
    info "下载 docker-compose.yml..."
    download "$BASE_URL/docker-compose.example.yml" "docker-compose.yml"
    success "docker-compose.yml 下载完成"
    
    info "下载 .env 模板..."
    download "$BASE_URL/env.zh.example" ".env"
    success ".env 下载完成"
    
    # Generate secure secrets
    info "生成安全密钥..."
    JWT_SECRET=$(generate_secret 32)
    SESSION_SECRET=$(generate_secret 32)
    
    # Update .env with generated secrets
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/JWT_SECRET=.*/JWT_SECRET=$JWT_SECRET/" .env
        sed -i '' "s/SESSION_SECRET=.*/SESSION_SECRET=$SESSION_SECRET/" .env
    else
        sed -i "s/JWT_SECRET=.*/JWT_SECRET=$JWT_SECRET/" .env
        sed -i "s/SESSION_SECRET=.*/SESSION_SECRET=$SESSION_SECRET/" .env
    fi
    success "密钥已生成并配置"
    
    # Set version if specified
    if [ "$VERSION" != "latest" ]; then
        info "设置版本: $VERSION"
        if [[ "$OSTYPE" == "darwin"* ]]; then
            sed -i '' "s/VERSION=.*/VERSION=$VERSION/" .env
        else
            sed -i "s/VERSION=.*/VERSION=$VERSION/" .env
        fi
    fi
    
    echo ""
    echo -e "${GREEN}+============================================================+${NC}"
    echo -e "${GREEN}|${NC}                        ${BLUE}安装完成!${NC}                           ${GREEN}|${NC}"
    echo -e "${GREEN}+============================================================+${NC}"
    echo ""
    echo "安装目录: $(pwd)"
    echo ""
    echo -e "${YELLOW}后续步骤:${NC}"
    echo ""
    echo "  1. 查看并自定义配置:"
    echo "     ${BLUE}nano .env${NC}"
    echo ""
    echo "  2. 启动应用:"
    echo "     ${BLUE}docker-compose up -d${NC}"
    echo ""
    echo "  3. 访问 Web 界面:"
    echo "     ${BLUE}http://localhost:15275${NC}"
    echo ""
    echo "  4. 默认登录凭据:"
    echo "     用户名: ${GREEN}admin${NC}"
    echo "     密码: ${GREEN}admin123!${NC}"
    echo ""
    echo -e "${YELLOW}[!] 重要: 首次登录后请务必修改默认密码!${NC}"
    echo ""
    
    # Ask if user wants to start now
    read -p "是否立即启动 Rust-Srec? [y/N] " -n 1 -r < /dev/tty
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        info "正在启动 Rust-Srec..."
        if docker compose version &>/dev/null 2>&1; then
            docker compose up -d
        else
            docker-compose up -d
        fi
        echo ""
        success "Rust-Srec 已启动!"
        echo ""
        echo "Web 界面: ${BLUE}http://localhost:15275${NC}"
        echo "API 文档: ${BLUE}http://localhost:12555/api/docs${NC}"
    else
        info "稍后可使用以下命令启动: docker-compose up -d"
    fi
}

# Run main function
main "$@"
