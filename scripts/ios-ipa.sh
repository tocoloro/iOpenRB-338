#!/usr/bin/env bash
# Builds an ad-hoc signed iPad .ipa of OpenRB-338, with an Audio Unit extension.
#
# Needs macOS with Xcode (the iOS SDK). A Linux machine cannot link this target.
# GitHub Actions runs it (.github/workflows/ios.yml). The signature is ad hoc (`-`):
# Impactor or SideServer replaces it with a free Apple ID. Nothing here uses a
# paid certificate.
#
# The extension is dist/OpenRB-338.app/PlugIns/OpenRB-338.appex. It has no view
# and no keyboard. A host loads it on a channel. Writes dist/OpenRB-338.ipa.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "scripts/ios-ipa.sh needs macOS and the iOS SDK (Xcode)." >&2
    exit 1
fi

export IPHONEOS_DEPLOYMENT_TARGET=15.0
SDKROOT="$(xcrun --sdk iphoneos --show-sdk-path)"
export SDKROOT
echo "SDKROOT=$SDKROOT"

rustup target add aarch64-apple-ios
cargo build --release --locked --target aarch64-apple-ios -p orb-ui -p orb-au

bin="target/aarch64-apple-ios/release/OpenRB-338"
lib="target/aarch64-apple-ios/release/liborb_au.a"
app="dist/OpenRB-338.app"
appex="dist/OpenRB-338.appex"
rm -rf "$app" "$appex" dist/Payload dist/OpenRB-338.ipa dist/OpenRBAudioUnit.o
mkdir -p "$app" "$appex"

# The Audio Unit extension. The .o is linked in full, so the factory class named
# by the plist is not dropped. The static library is the engine. An appex starts
# at NSExtensionMain; it is not an application and has no main.
clang -fobjc-arc -fmodules -fapplication-extension -isysroot "$SDKROOT" -arch arm64 \
    -miphoneos-version-min=15.0 \
    -c ios/au/OpenRBAudioUnit.m -o dist/OpenRBAudioUnit.o
clang -fobjc-arc -fmodules -fapplication-extension -isysroot "$SDKROOT" -arch arm64 \
    -miphoneos-version-min=15.0 \
    -F"$SDKROOT/System/Library/Frameworks" \
    -Wl,-e,_NSExtensionMain \
    dist/OpenRBAudioUnit.o "$lib" \
    -framework Foundation -framework AVFoundation -framework AudioToolbox \
    -framework CoreAudio \
    -o "$appex/OpenRB-338"
cp ios/au/Info.plist "$appex/Info.plist"
codesign --force --sign - --timestamp=none "$appex/OpenRB-338"
codesign --force --sign - --timestamp=none "$appex"

cp "$bin" "$app/OpenRB-338"
cp ios/Info.plist "$app/Info.plist"
mkdir -p "$app/PlugIns"
cp -R "$appex" "$app/PlugIns/OpenRB-338.appex"
# Four-char creator code. Old installers look for this file.
printf 'APPL????' > "$app/PkgInfo"
chmod 755 "$app/OpenRB-338"

codesign --force --sign - --timestamp=none "$app/OpenRB-338"
codesign --force --sign - --timestamp=none "$app"
codesign --verify --verbose=2 "$app"

mkdir -p dist/Payload
cp -R "$app" dist/Payload/
(
    cd dist
    zip -r -y -q OpenRB-338.ipa Payload
)
rm -rf dist/Payload
echo "Wrote dist/OpenRB-338.ipa"
