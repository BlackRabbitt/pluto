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

CDPATH='' cd -- "$(dirname -- "$0")"
rust_version=1.100.0
if ! rustup run "$rust_version" rustc --version >/dev/null 2>&1; then
    rustup toolchain install "$rust_version"
fi
applications="$HOME/Applications"
launcher="$applications/Pluto.app"
bundle_id=com.blackrabbitt.pluto
mkdir -p "$applications"
staging=$(mktemp -d "$applications/.pluto-install.XXXXXX")
trap 'if [ -d "$staging/previous.app" ] && [ ! -e "$launcher" ] && [ ! -L "$launcher" ]; then
    mv "$staging/previous.app" "$launcher" || exit 1
fi
rm -rf "$staging"' EXIT
trap 'exit 1' HUP INT TERM

is_pluto_launcher() {
    [ ! -L "$launcher" ] || return 1
    info="$launcher/Contents/Info.plist"
    if [ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$info" 2>/dev/null)" = "$bundle_id" ]; then
        return 0
    fi

    # Match the script from older installers before replacing an AppleScript app.
    [ "$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$info" 2>/dev/null)" = applet ] || return 1
    /usr/bin/osacompile -o "$staging/legacy.scpt" <<'APPLESCRIPT' || return 1
set plutoPath to (POSIX path of (path to home folder)) & ".cargo/bin/pluto"
do shell script "/usr/bin/nohup " & quoted form of plutoPath & " </dev/null >/dev/null 2>&1 &"
APPLESCRIPT
    /usr/bin/osadecompile "$staging/legacy.scpt" > "$staging/expected.applescript" &&
        /usr/bin/osadecompile "$launcher/Contents/Resources/Scripts/main.scpt" > "$staging/installed.applescript" &&
        cmp -s "$staging/expected.applescript" "$staging/installed.applescript"
}

if { [ -e "$launcher" ] || [ -L "$launcher" ]; } && ! is_pluto_launcher; then
    printf 'Move the existing app at %s aside and rerun the installer.\n' "$launcher" >&2
    exit 1
fi

rustup run "$rust_version" cargo install --locked --path . --root "$HOME/.cargo"

staged_launcher="$staging/Pluto.app"
contents="$staged_launcher/Contents"
iconset="$staging/Pluto.iconset"
mkdir -p "$contents/MacOS" "$contents/Resources" "$iconset"
cp "$HOME/.cargo/bin/pluto-desktop" "$contents/MacOS/Pluto"

if [ "$(uname -m)" = arm64 ]; then
    vm_resources="$contents/Resources/pluto"
    mkdir -p "$vm_resources/vm"
    cp ../pluto/Cargo.toml ../pluto/Cargo.lock "$vm_resources/"
    cp -R ../pluto/src "$vm_resources/"
    cp ../pluto/vm/vm ../pluto/vm/lima.yaml ../pluto/vm/pluto.init ../pluto/vm/setup-vm.sh "$vm_resources/vm/"
fi

for size in 16 32 128 256 512; do
    name="icon_${size}x${size}"
    /usr/bin/sips -z "$size" "$size" assets/pluto-icon.png --out "$iconset/$name.png" >/dev/null
    double_size=$((size * 2))
    /usr/bin/sips -z "$double_size" "$double_size" assets/pluto-icon.png --out "$iconset/$name@2x.png" >/dev/null
done
/usr/bin/iconutil -c icns -o "$contents/Resources/Pluto.icns" "$iconset"

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleExecutable</key>
    <string>Pluto</string>
    <key>CFBundleName</key>
    <string>Pluto Desktop</string>
    <key>CFBundleDisplayName</key>
    <string>Pluto Desktop</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleIconFile</key>
    <string>Pluto.icns</string>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Pluto Desktop can connect to model and MCP servers you configure on your local network.</string>
</dict>
</plist>
PLIST
/usr/bin/codesign --force --sign "${PLUTO_SIGNING_IDENTITY:--}" "$staged_launcher"

if [ -d "$launcher" ]; then
    mv "$launcher" "$staging/previous.app"
fi
mv -n "$staged_launcher" "$applications/"
test ! -e "$staged_launcher"

printf '\nLauncher: %s\n' "$launcher"
printf '%s\n' 'Open Pluto Desktop from Spotlight or your Applications folder.'
