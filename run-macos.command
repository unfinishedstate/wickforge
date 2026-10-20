#!/bin/bash
set -euo pipefail

# Finder does not necessarily inherit the shell's Rust PATH.
if ! command -v cargo >/dev/null 2>&1; then
    rust_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
    if [[ -x "$rust_bin/cargo" ]]; then
        export PATH="$rust_bin:$PATH"
    else
        printf '%s\n' 'Rust 1.89 or newer is required. Install it from https://rustup.rs, then run this launcher again.' >&2
        exit 1
    fi
fi

if [[ "$(uname -s)" == "Darwin" ]] && ! xcode-select -p >/dev/null 2>&1; then
    printf '%s\n' 'Xcode Command Line Tools are required. Run: xcode-select --install' >&2
    exit 1
fi

project_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$project_root"

printf '%s\n' 'Building and starting Wickforge. The first build can take several minutes.'
exec cargo run --release --locked --package mexc-chart --bin mexc-chart -- "$@"
