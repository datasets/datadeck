#!/bin/bash
# Package and exercise the exact disk image that people will download.
set -euo pipefail

die() { echo "$*" >&2; exit 1; }
[[ $# == 4 ]] || die "Usage: $0 preview|release BUNDLE ARCH OUTPUT_DIRECTORY"
mode=$1; bundle=$2; arch=$3; output=$4
[[ $mode == release || $mode == preview ]] || die 'Choose release or preview.'
[[ $arch == arm64 || $arch == x86_64 ]] || die 'Expected arm64 or x86_64.'
[[ $(uname -m) == "$arch" ]] || die "This verification requires a native $arch Mac."
[[ -d $bundle ]] || die "Missing application: $bundle"
if [[ $mode == release ]]; then
    for credential in APPLE_DEVELOPER_ID_P12_BASE64 APPLE_DEVELOPER_ID_P12_PASSWORD APPLE_NOTARY_KEY_BASE64 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID; do
        [[ -n ${!credential:-} ]] || die "Missing $credential; signed releases require all five credentials."
    done
fi

version=$(/usr/bin/plutil -extract CFBundleShortVersionString raw "$bundle/Contents/Info.plist")
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Expected a three-part application version, found $version."
mkdir -p "$output"
output=$(cd "$output" && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/datadeck-dmg.XXXXXXXX")
keychain="$work/release.keychain-db"
mount="$work/mount"
mounted=false
cleanup() {
    if $mounted; then /usr/bin/hdiutil detach "$mount" >/dev/null || /usr/bin/hdiutil detach -force "$mount" >/dev/null; fi
    if [[ -f $keychain ]]; then /usr/bin/security delete-keychain "$keychain" >/dev/null; fi
    rm -rf "$work"
}
trap cleanup EXIT
mkdir "$work/image" "$mount"
/usr/bin/ditto "$bundle" "$work/image/DataDeck.app"
ln -s /Applications "$work/image/Applications"
app="$work/image/DataDeck.app"
name="DataDeck-$version-macos-$arch"
if [[ $mode == preview ]]; then
    name="$name-unsigned-preview"
    /usr/bin/codesign --force --sign - "$app"
else
    # Restrict credential material to this temporary directory. Never enable shell tracing.
    (
        umask 077
        printf '%s' "$APPLE_DEVELOPER_ID_P12_BASE64" | /usr/bin/base64 -D > "$work/identity.p12"
        printf '%s' "$APPLE_NOTARY_KEY_BASE64" | /usr/bin/base64 -D > "$work/notary.p8"
    )
    password=$(/usr/bin/openssl rand -hex 32)
    /usr/bin/security create-keychain -p "$password" "$keychain"
    /usr/bin/security set-keychain-settings -lut 7200 "$keychain"
    /usr/bin/security unlock-keychain -p "$password" "$keychain"
    /usr/bin/security import "$work/identity.p12" -P "$APPLE_DEVELOPER_ID_P12_PASSWORD" -k "$keychain" -T /usr/bin/codesign
    /usr/bin/security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
    rm "$work/identity.p12"
    unset password APPLE_DEVELOPER_ID_P12_BASE64 APPLE_DEVELOPER_ID_P12_PASSWORD APPLE_NOTARY_KEY_BASE64
    identity=$(/usr/bin/security find-identity -v -p codesigning "$keychain" | awk '/"Developer ID Application:/ {print $2}')
    [[ $identity =~ ^[a-fA-F0-9]{40}$ ]] || die 'A single valid Developer ID Application identity and its certificate chain are required.'
    /usr/bin/codesign --force --sign "$identity" --keychain "$keychain" --options runtime --timestamp "$app"
fi
/usr/bin/codesign --verify --deep --strict --verbose=2 "$app"

dmg="$work/$name.dmg"
/usr/bin/hdiutil create -volname DataDeck -srcfolder "$work/image" -format UDZO -fs HFS+ "$dmg"
if [[ $mode == release ]]; then
    /usr/bin/codesign --force --sign "$identity" --keychain "$keychain" --timestamp "$dmg"
    auth=(--key "$work/notary.p8" --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID")
    submitted=true
    xcrun notarytool submit "$dmg" "${auth[@]}" --wait --timeout 60m --output-format json > "$work/result.json" || submitted=false
    cat "$work/result.json"
    if ! $submitted || [[ $(/usr/bin/plutil -extract status raw "$work/result.json") != Accepted ]]; then
        id=$(/usr/bin/plutil -extract id raw "$work/result.json" 2>/dev/null || true)
        if [[ $id =~ ^[a-fA-F0-9-]{36}$ ]]; then xcrun notarytool log "$id" "${auth[@]}" || true; fi
        die 'Apple did not accept this image.'
    fi
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
    /usr/bin/codesign --verify --strict "$dmg"
    /usr/sbin/spctl --assess --type open --context context:primary-signature --verbose=4 "$dmg"
fi
/usr/bin/hdiutil verify "$dmg"
/usr/bin/hdiutil attach "$dmg" -readonly -nobrowse -noautoopen -mountpoint "$mount"
mounted=true
installed="$mount/DataDeck.app"
plist="$installed/Contents/Info.plist"
binary="$installed/Contents/MacOS/datadeck"
/usr/bin/plutil -lint "$plist"
[[ $(readlink "$mount/Applications") == /Applications ]] || die 'Missing Applications installation shortcut.'
[[ $(/usr/bin/plutil -extract CFBundleIdentifier raw "$plist") == org.datasets.datadeck ]] || die 'Unexpected bundle identifier.'
[[ $(/usr/bin/plutil -extract CFBundleShortVersionString raw "$plist") == "$version" ]] || die 'Bundle version changed.'
[[ $(/usr/bin/lipo -archs "$binary") == "$arch" ]] || die 'Wrong CPU architecture in the image.'
deployment=$(/usr/bin/vtool -show-build "$binary" | awk '/minos/ {print $2; exit}')
[[ -n $deployment && $(/usr/bin/plutil -extract LSMinimumSystemVersion raw "$plist") == "$deployment" ]] || die 'The declared minimum macOS version differs from the binary.'
if [[ -n ${MACOSX_DEPLOYMENT_TARGET:-} && $deployment != "$MACOSX_DEPLOYMENT_TARGET" ]]; then
    die "The binary targets macOS $deployment; expected $MACOSX_DEPLOYMENT_TARGET."
fi
[[ -s "$installed/Contents/Resources/DataDeck.icns" ]] || die 'Missing application icon.'
/usr/bin/codesign --verify --deep --strict --verbose=2 "$installed"
external=$(/usr/bin/otool -L "$binary" | awk 'NR>1 && $1 !~ /^\/System\/Library\// && $1 !~ /^\/usr\/lib\// {print $1}')
[[ -z $external ]] || die "The executable requires libraries outside macOS: $external"
if [[ $mode == release ]]; then
    /usr/sbin/spctl --assess --type execute --verbose=4 "$installed"
fi
printf 'name,score\nA,3\nB,4\n' > "$work/input.csv"
printf 'SELECT SUM(score) AS total FROM data;\n' > "$work/recipe.sql"
"$binary" run "$work/recipe.sql" "$work/input.csv" > "$work/result.csv"
printf 'total\n7\n' > "$work/expected.csv"
diff -u "$work/expected.csv" "$work/result.csv"
/usr/bin/hdiutil detach "$mount"
mounted=false
mv "$dmg" "$output/$name.dmg"
(cd "$output" && /usr/bin/shasum -a 256 "$name.dmg" > "$name.dmg.sha256" && /usr/bin/shasum -a 256 -c "$name.dmg.sha256")
