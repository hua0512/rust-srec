#!/usr/bin/env bash
set -euo pipefail

apt_options=(
  -o Acquire::Retries=1
  -o Acquire::http::Timeout=15
  -o Acquire::https::Timeout=15
)

download_packages() {
  sudo timeout --kill-after=10s 90s apt-get "${apt_options[@]}" update &&
    sudo timeout --kill-after=10s 180s apt-get "${apt_options[@]}" \
      install -y --no-install-recommends --download-only "$@"
}

if ! download_packages "$@"; then
  echo "::warning::Ubuntu package download failed; retrying with archive.ubuntu.com"
  # Hosted runners may select Azure through either APT sources or a mirror list.
  sudo find /etc/apt -maxdepth 2 -type f \
    \( -name sources.list -o -name '*.list' -o -name '*.sources' -o -name apt-mirrors.txt \) \
    -exec sed -i \
      's|://azure\.archive\.ubuntu\.com/ubuntu|://archive.ubuntu.com/ubuntu|g' {} +
  download_packages "$@"
fi

# Bound network work separately so a timeout never interrupts dpkg installation.
sudo apt-get install -y --no-install-recommends --no-download "$@"
