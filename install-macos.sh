#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'This installer requires macOS.' >&2
    exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
    printf '%s\n' 'Install Rustup from https://rustup.rs/ and rerun this script.' >&2
    exit 1
fi
if ! xcode-select -p >/dev/null 2>&1; then
    printf '%s\n' 'Run xcode-select --install, finish the installation, and rerun this script.' >&2
    exit 1
fi

cd "$(dirname "$0")"
rust_version=1.100.0
if ! rustup run "$rust_version" rustc --version >/dev/null 2>&1; then
    rustup toolchain install "$rust_version"
fi
rustup run "$rust_version" cargo install --locked --path . --root "$HOME/.cargo"

applications="$HOME/Applications"
launcher="$applications/Pluto.app"
mkdir -p "$applications"
if [ ! -e "$launcher" ] && [ ! -L "$launcher" ]; then
    staging=$(mktemp -d "$applications/.pluto-install.XXXXXX")
    trap 'rm -rf "$staging"' EXIT
    trap 'exit 1' HUP INT TERM
    /usr/bin/osacompile -o "$staging/Pluto.app" <<'APPLESCRIPT'
set plutoPath to (POSIX path of (path to home folder)) & ".cargo/bin/pluto"
do shell script "/usr/bin/nohup " & quoted form of plutoPath & " </dev/null >/dev/null 2>&1 &"
APPLESCRIPT
    mv -n "$staging/Pluto.app" "$applications/"
fi

printf '\n%s\n' 'Pluto is ready. Open Pluto from Spotlight or your Applications folder.'
