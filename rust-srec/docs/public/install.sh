#!/usr/bin/env bash
#
# Rust-Srec Install Loader
# Bootstrap that picks a localized installer and runs it.
# https://github.com/hua0512/rust-srec
#
# Usage:
#   curl -fsSL https://docs.srec.rs/install.sh | bash
#   wget -qO- https://docs.srec.rs/install.sh | bash
#
# For the Chinese installer:
#   curl -fsSL https://docs.srec.rs/install.sh | SREC_LANG=zh bash
#
# With custom parameters:
#   curl -fsSL https://docs.srec.rs/install.sh | RUST_SREC_DIR=/opt/rust-srec VERSION=v0.5.1 bash

set -euo pipefail

BASE_URL="https://docs.srec.rs"

RED='\033[0;31m'
NC='\033[0m'
error() { printf "${RED}[ERROR]${NC} %s\n" "$*" >&2; }

# Language selection mirrors install.ps1: explicit SREC_LANG wins, otherwise
# fall back to the locale environment, otherwise English.
use_zh=0
if [ "${SREC_LANG:-}" = "zh" ]; then
    use_zh=1
elif [ -z "${SREC_LANG:-}" ]; then
    case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in
        zh* | ZH*) use_zh=1 ;;
    esac
fi

if [ "$use_zh" -eq 1 ]; then
    script_name="docker-install-zh.sh"
    msg_no_tool="未找到 curl 或 wget，请先安装其中之一。"
    msg_failed="下载安装脚本失败："
    msg_manual="可手动下载后运行："
else
    script_name="docker-install.sh"
    msg_no_tool="Neither curl nor wget was found; install one of them first."
    msg_failed="Failed to download the installer:"
    msg_manual="Download it and run it manually:"
fi

script_url="$BASE_URL/$script_name"

fail() {
    error "$msg_failed $script_url"
    error "$msg_manual $script_url"
    exit 1
}

tmp_script="$(mktemp "${TMPDIR:-/tmp}/rust-srec-install.XXXXXX")" || exit 1
trap 'rm -f "$tmp_script"' EXIT INT TERM

if command -v curl > /dev/null 2>&1; then
    curl -fsSL "$script_url" -o "$tmp_script" || fail
elif command -v wget > /dev/null 2>&1; then
    wget -qO "$tmp_script" "$script_url" || fail
else
    error "$msg_no_tool"
    exit 1
fi

# A proxy or error page can return HTTP 200 with a body that is not the script.
[ -s "$tmp_script" ] || fail
head -n 1 "$tmp_script" | grep -q '^#!' || fail

# The installer prompts via /dev/tty, so running it here stays interactive even
# when this bootstrap itself arrived through a pipe.
bash "$tmp_script" "$@"
