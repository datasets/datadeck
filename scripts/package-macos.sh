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
target_dir=$(cargo metadata --format-version 1 --no-deps | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
bundle="$target_dir/$profile/DataDeck.app"
contents="$bundle/Contents"

mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 755 "$target_dir/$profile/datadeck" "$contents/MacOS/datadeck"
install -m 644 assets/DataDeck.icns "$contents/Resources/DataDeck.icns"
install -m 644 scripts/Info.plist "$contents/Info.plist"
install -m 644 assets/fonts/OFL.txt "$contents/Resources/IBM-Plex-License.txt"
/usr/bin/plutil -lint "$contents/Info.plist"
/usr/bin/codesign --force --sign - "$bundle"
/usr/bin/codesign --verify --strict "$bundle"
echo "$bundle"
