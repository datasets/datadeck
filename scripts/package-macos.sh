#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
case "${1-}" in
    --dev) profile=debug; shift ;;
    "") profile=release ;;
    *) echo 'Usage: scripts/package-macos.sh [--dev]' >&2; exit 2 ;;
esac
if [ "$#" != 0 ]; then
    echo 'Usage: scripts/package-macos.sh [--dev]' >&2
    exit 2
fi
if [ "$(uname -s)" != Darwin ]; then
    echo 'Build the macOS application on a Mac.' >&2
    exit 1
fi

if [ "$profile" = release ]; then
    cargo build --locked --release --bin datadeck
else
    cargo build --locked --bin datadeck
fi
metadata=$(cargo metadata --locked --format-version 1 --no-deps)
target_dir=$(printf '%s' "$metadata" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
version=$(printf '%s' "$metadata" | /usr/bin/python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "datadeck"))')
bundle="$target_dir/$profile/DataDeck.app"
contents="$bundle/Contents"

mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 755 "$target_dir/$profile/datadeck" "$contents/MacOS/datadeck"
install -m 644 assets/DataDeck.icns "$contents/Resources/DataDeck.icns"
install -m 644 scripts/Info.plist "$contents/Info.plist"
install -m 644 assets/fonts/OFL.txt "$contents/Resources/IBM-Plex-License.txt"
min_os=$(/usr/bin/vtool -show-build "$contents/MacOS/datadeck" | awk '$1 == "minos" || $1 == "version" {print $2; exit}')
/usr/bin/plutil -replace CFBundleShortVersionString -string "$version" "$contents/Info.plist"
/usr/bin/plutil -replace CFBundleVersion -string "$version" "$contents/Info.plist"
if [ -n "$min_os" ]; then
    /usr/bin/plutil -insert LSMinimumSystemVersion -string "$min_os" "$contents/Info.plist"
fi
/usr/bin/plutil -lint "$contents/Info.plist"
/usr/bin/codesign --force --sign - "$bundle"
/usr/bin/codesign --verify --strict "$bundle"
echo "$bundle"
