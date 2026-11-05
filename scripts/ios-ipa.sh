#!/usr/bin/env bash
# Builds an ad-hoc signed iPad .ipa of OpenRB-338, the application only.
#
# Needs macOS with Xcode (the iOS SDK). A Linux machine cannot link this target.
# GitHub Actions runs it (.github/workflows/ios.yml). The signature is ad hoc (`-`):
# Impactor or SideServer replaces it with a free Apple ID. Nothing here uses a
# paid certificate.
#
# Writes dist/OpenRB-338.ipa. No Audio Unit is packaged.
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
cargo build --release --locked --target aarch64-apple-ios -p orb-ui

bin="target/aarch64-apple-ios/release/OpenRB-338"
app="dist/OpenRB-338.app"
rm -rf "$app" dist/Payload dist/OpenRB-338.ipa
mkdir -p "$app"

cp "$bin" "$app/OpenRB-338"
cp ios/Info.plist "$app/Info.plist"
cp ios/AppIcon76x76.png ios/AppIcon76x76@2x.png "$app/"
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
