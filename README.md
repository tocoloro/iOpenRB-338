<p align="center">
  <img src="assets/readme/logo.gif" alt="OpenRB-338" width="378">
</p>

<div align="center">

# OpenRB-338 for iPad

</div>

An unofficial iPad build of [OpenRB-338](https://github.com/pingw33n/openrb-338),
the reimplementation of ReBirth RB-338 by [pingw33n](https://github.com/pingw33n).

The engine, the panel, the song and mod support, and the desktop and command-line
applications are that project. This repository does not replace it. It packages the
same program for an iPad and describes one way to install that package. Bugs in the
sound and the panel belong upstream. The iPad packaging is mine.

## What OpenRB-338 is

OpenRB-338 rebuilds ReBirth RB-338 2.01, the 1997 software synth of two TB-303 bass
lines, a TR-808 and a TR-909. The exported loop of a song matches the original's
"Export Loop as Audio File". The panel is the whole machine: the two 303s, the 808
and the 909, the mixer and the effects, in pattern and song mode. It opens ReBirth
songs (`.rbs`) and drum mods (`.rbm`). The standard kit is built in, so no file from
the original program is required.

The full description, the desktop downloads, the song collection and the mod
collection are on the original repository:
[pingw33n/openrb-338](https://github.com/pingw33n/openrb-338).

## The iPad build

The iPad application is the same panel, locked to landscape. The whole machine stays
on screen. The status bar is hidden, so the menu sits at the top. The panel sits a
few points above the bottom edge, so the TR-909 step row can be pressed.

Several fingers can hold controls at once. Options, Large panel draws the panel at
twice the size. Two fingers on empty space move it, and fingers on the controls
still play them. That option is off unless you turn it on. Export on the iPad writes WAV and FLAC. Songs copied into the app's Documents
folder, through the Files app, are what File, Open can load.

## In a host

The same install includes an Audio Unit named OpenRB-338. It is a generator, so
AUM lists it in a channel's source slot, not among the apps. That channel's
effects process what it plays. It has no screen and no keyboard. While the host is playing, the pattern follows the host's tempo
and does not drift. Opened as the application, the clock is the song's own.

The Audio Unit plays a short TR-909 pattern on the built-in kit. The standard kit has
no demo song, and the song open in the application stays there: a free Apple ID cannot
hand it to the plugin.

There is no App Store listing and no install package in Releases. The `.ipa` is the
artifact `OpenRB-338-ipad` of the
[ios workflow](https://github.com/tocoloro/iOpenReverb-338/actions/workflows/ios.yml).

## Install it on an iPad

This needs a free Apple ID, a USB cable and a computer. It does not need the paid
Apple Developer Program. The free certificate lasts 7 days. When it expires the app
stops opening until you install it again.

1. Download `OpenRB-338.ipa` from the latest successful run of the ios workflow. The
   artifact is named `OpenRB-338-ipad`.
2. On the iPad, set a passcode. On iPadOS 16 and newer, turn on Developer Mode
   (Settings, Privacy & Security, Developer Mode) and let the iPad restart.
3. Connect the iPad, unlock it, and tap Trust.
4. On the computer, install the `.ipa` with
   [Impactor](https://github.com/khcrysalis/Impactor) (on Linux, the Plume Impactor
   flatpak), signed in with the Apple ID.
5. On the iPad, open Settings, General, VPN & Device Management and trust the
   developer certificate. Then open OpenRB-338.

The build asks for iPadOS 15 or newer. Copy songs into the app from the Files app.
The built-in drum kit is already inside the app.

## Disclaimer

I am not responsible for this install. That includes an Apple ID warning, a
certificate that expires, a failed install, or a song or recording lost while trying
it. This is not an official build, it is not signed for the App Store, and it is not
maintained by pingw33n.

It has been tried on one iPad: 7th generation (2019, model iPad7,11, A10, home
button), iPadOS 18.7.3, held in landscape. I have not tried any other model or any
other system version. If it does not run on yours, I have no fix to offer.

ReBirth RB-338 is a trademark of Propellerhead Software (now Reason Studios).
OpenRB-338 is not affiliated with or endorsed by them.

## License

The program is free software under the [GNU General Public License v3.0](LICENSE)
(GPL-3.0-only). Copyright (C) 2026 the OpenRB-338 authors.

The built-in standard kit (`assets/Standard.rbm`) holds ReBirth's own drum samples
and panel pictures. See [assets/README.md](assets/README.md).
