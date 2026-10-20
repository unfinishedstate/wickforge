#!/bin/bash
set -euo pipefail

# Load the full launch routine before pulling: Git may replace this script.
main() {
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

    if ! command -v git >/dev/null 2>&1; then
        printf '%s\n' 'Git is required to update Wickforge before launch.' >&2
        exit 1
    fi
    if [[ ! -e .git ]]; then
        printf '%s\n' 'Automatic updates require a Git clone, not a downloaded source ZIP. Clone https://github.com/unfinishedstate/wickforge.git and run its launcher.' >&2
        exit 1
    fi
    status="$(git status --porcelain --untracked-files=no)"
    if [[ -n "$status" ]]; then
        printf '%s\n' 'Tracked files have local changes. Commit or stash them yourself before launching; nothing was overwritten.' >&2
        exit 1
    fi

    launcher_before="$(git hash-object -- run-macos.command)"
    printf '%s\n' 'Updating Wickforge from the current branch upstream (fast-forward only).'
    git pull --ff-only

    if [[ "$(git hash-object -- run-macos.command)" != "$launcher_before" ]]; then
        printf '%s\n' 'Launcher updated; restarting it before building.'
        exec /bin/bash "$project_root/run-macos.command" "$@"
    fi

    printf '%s\n' 'Building and starting Wickforge. The first build can take several minutes.'
    exec cargo run --release --locked --package mexc-chart --bin mexc-chart -- "$@"
}

main "$@"
