#!/usr/bin/env bash
#
# Rust-Srec Installation Script
# https://github.com/hua0512/rust-srec
#
# Usage:
#   curl -fsSL https://docs.srec.rs/docker-install.sh | bash
#   wget -qO- https://docs.srec.rs/docker-install.sh | bash
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
info() { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Generate secure random string
generate_secret() {
    local length="${1:-32}"
    if command -v openssl &>/dev/null; then
        openssl rand -hex "$length"
    elif [ -r /dev/urandom ]; then
        head -c "$length" /dev/urandom | od -An -tx1 | tr -d ' \n'
    else
        # Fallback using $RANDOM (less secure, but works everywhere)
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
        missing+=("curl or wget")
    fi
    
    if [ ${#missing[@]} -gt 0 ]; then
        error "Missing required tools: ${missing[*]}"
        echo ""
        echo "Please install the following before running this script:"
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
        error "Neither curl nor wget found"
        exit 1
    fi
}

# Main installation
main() {
    echo ""
    echo -e "${GREEN}+============================================================+${NC}"
    echo -e "${GREEN}|${NC}          ${BLUE}Rust-Srec Installation Script${NC}                     ${GREEN}|${NC}"
    echo -e "${GREEN}|${NC}          Automatic Streaming Recorder                       ${GREEN}|${NC}"
    echo -e "${GREEN}+============================================================+${NC}"
    echo ""
    
    # Check requirements
    info "Checking requirements..."
    check_requirements
    success "All requirements met"
    
    # Version selection
    if [ "$VERSION" = "latest" ]; then
        echo ""
        echo -e "${YELLOW}Select version to install:${NC}"
        echo "  1) latest  - Stable release (recommended)"
        echo "  2) dev     - Development build (bleeding edge)"
        echo ""
        read -p "Enter choice [1]: " version_choice < /dev/tty
        case "$version_choice" in
            2|dev)
                VERSION="dev"
                ;;
            *)
                VERSION="latest"
                ;;
        esac
        info "Selected version: $VERSION"
    fi
    
    # Create installation directory
    info "Creating installation directory: $INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
    cd "$INSTALL_DIR"
    
    # Download configuration files
    info "Downloading docker-compose.yml..."
    download "$BASE_URL/docker-compose.example.yml" "docker-compose.yml"
    success "docker-compose.yml downloaded"
    
    info "Downloading .env template..."
    download "$BASE_URL/env.example" ".env"
    success ".env downloaded"
    
    # Generate secure secrets
    info "Generating secure secrets..."
    JWT_SECRET=$(generate_secret 32)
    SESSION_SECRET=$(generate_secret 32)
    
    # Update .env with generated secrets
    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS requires different sed syntax
        sed -i '' "s/JWT_SECRET=.*/JWT_SECRET=$JWT_SECRET/" .env
        sed -i '' "s/SESSION_SECRET=.*/SESSION_SECRET=$SESSION_SECRET/" .env
    else
        sed -i "s/JWT_SECRET=.*/JWT_SECRET=$JWT_SECRET/" .env
        sed -i "s/SESSION_SECRET=.*/SESSION_SECRET=$SESSION_SECRET/" .env
    fi
    success "Secrets generated and configured"
    
    # Set version if specified
    if [ "$VERSION" != "latest" ]; then
        info "Setting version to: $VERSION"
        if [[ "$OSTYPE" == "darwin"* ]]; then
            sed -i '' "s/VERSION=.*/VERSION=$VERSION/" .env
        else
            sed -i "s/VERSION=.*/VERSION=$VERSION/" .env
        fi
    fi
    
    echo ""
    echo -e "${GREEN}+============================================================+${NC}"
    echo -e "${GREEN}|${NC}              ${BLUE}Installation Complete!${NC}                       ${GREEN}|${NC}"
    echo -e "${GREEN}+============================================================+${NC}"
    echo ""
    echo "Installation directory: $(pwd)"
    echo ""
    echo -e "${YELLOW}Next steps:${NC}"
    echo ""
    echo "  1. Review and customize your configuration:"
    echo "     ${BLUE}nano .env${NC}"
    echo ""
    echo "  2. Start the application:"
    echo "     ${BLUE}docker-compose up -d${NC}"
    echo ""
    echo "  3. Access the web interface:"
    echo "     ${BLUE}http://localhost:15275${NC}"
    echo ""
    echo "  4. Default login credentials:"
    echo "     Username: ${GREEN}admin${NC}"
    echo "     Password: ${GREEN}admin123!${NC}"
    echo ""
    echo -e "${YELLOW}[!] Important:${NC} Change the default password after first login!"
    echo ""
    
    # Ask if user wants to start now
    read -p "Would you like to start Rust-Srec now? [y/N] " -n 1 -r < /dev/tty
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        info "Starting Rust-Srec..."
        if docker compose version &>/dev/null 2>&1; then
            docker compose up -d
        else
            docker-compose up -d
        fi
        echo ""
        success "Rust-Srec is now running!"
        echo ""
        echo "Web Interface: ${BLUE}http://localhost:15275${NC}"
        echo "API Docs:      ${BLUE}http://localhost:12555/api/docs${NC}"
    else
        info "You can start Rust-Srec later with: docker-compose up -d"
    fi
}

# Run main function
main "$@"
