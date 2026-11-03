# OpenRB-338 on an iPad, without a paid Apple account

Personal install only. The `.ipa` is built on a GitHub Actions macOS runner
(`.github/workflows/ios.yml`, started by hand) and installed from Linux with
[SideServer](https://github.com/SideStore/SideServer-for-Linux). SideServer signs it
with a free Apple ID. The certificate lasts 7 days. SideStore, once it is on the iPad,
refreshes that certificate over Wi-Fi.

A free Apple ID allows 3 apps at a time (SideStore counts as one) and 10 App IDs a
week. Use an Apple ID that is not your main one. The iPad needs a passcode, iPadOS 15
or newer, and a USB cable for the first install.

## 1. Build the IPA

Push this branch to a GitHub repository you control (not `pingw33n/openrb-338` unless
that is yours). Open Actions, choose the workflow `ios`, and run it. When it finishes,
download the artifact `OpenRB-338-ipad` (`OpenRB-338.ipa`).

The runner compiles `orb-ui` for `aarch64-apple-ios`, wraps the binary with
`ios/Info.plist` and ad-hoc signs it. `scripts/ios-ipa.sh` is that packaging. It does
not run on Linux: the iOS SDK exists only on macOS.

## 2. Install SideServer on Ubuntu 22.04

Install the `.deb` from the SideServer releases (Python 3.10). Or, without the package:

```bash
sudo apt-get install binutils python3-pip git gir1.2-appindicator3-0.1 usbmuxd \
  libimobiledevice6 libimobiledevice-utils wget curl \
  libavahi-compat-libdnssd-dev zlib1g-dev unzip usbutils
git clone https://github.com/SideStore/SideServer-for-Linux
cd SideServer-for-Linux
python3 main.py
```

Unlock the iPad, connect it, and tap Trust. In SideServer, sign in with the Apple ID
and sideload `OpenRB-338.ipa`. The bundle id is `local.irebirth.openrb338`. The app is
iPad only.

Songs copied into the app's Documents folder (Finder or the Files app: the plist sets
`UIFileSharingEnabled`) are what the in-app browser can open. The built-in drum kit is
inside the binary. Export on iPad writes WAV and FLAC; Ogg is desktop only, because its
encoder is a C library.

## 3. Keep it past 7 days

Sideload SideStore itself from the same SideServer window, then open SideStore on the
iPad and let it refresh OpenRB-338. The computer is needed again only if a refresh is
missed and the app stops launching. Refreshing does not need the 99 USD program.
