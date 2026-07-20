#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(CDPATH= cd -- "${script_dir}/.." && pwd)
budget_file="${repo_root}/.github/public-api/rust-srec.max-items"

read -r max_items < "${budget_file}"
if [[ ! "${max_items}" =~ ^[0-9]+$ ]]; then
    echo "Invalid rust-srec public API budget: ${max_items}" >&2
    exit 2
fi

cd "${repo_root}"
current_items=$(
    cargo public-api \
        --manifest-path rust-srec/Cargo.toml \
        --all-features \
        -sss \
        --color never \
        | awk 'NF { count += 1 } END { print count + 0 }'
)

if (( current_items > max_items )); then
    echo "rust-srec public API expanded: ${current_items} items exceeds budget ${max_items}." >&2
    echo "Make new exports crate-private or update the reviewed budget intentionally." >&2
    exit 1
fi

echo "rust-srec public API: ${current_items}/${max_items} items"
