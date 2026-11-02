# Podroid

[简体中文](README.md) | [English](README.en.md)

[![GitHub Release](https://img.shields.io/github/v/release/shawnw67/Podroid?style=flat&logo=github&color=1f6feb)](https://github.com/shawnw67/Podroid/releases)
[![GitHub Stars](https://img.shields.io/github/stars/shawnw67/Podroid?style=flat&logo=github&color=gold)](https://github.com/shawnw67/Podroid)
[![GitHub Issues](https://img.shields.io/github/issues/shawnw67/Podroid?style=flat&logo=github&color=lightgrey)](https://github.com/shawnw67/Podroid/issues)
[![License](https://img.shields.io/github/license/shawnw67/Podroid?style=flat&logo=github&color=blue)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Android-blue?style=flat)](https://github.com/shawnw67/Podroid/releases)

**Podroid** is an unofficial fork of [touchHLE](https://github.com/touchHLE/touchHLE), a high-level emulator for early iOS apps. This fork adds game-specific compatibility fixes, an Android launcher, save import/export, device profiles, and native controller support.

Maintained by [shawnw67](https://github.com/shawnw67). This fork includes AI-assisted code and is maintained independently of upstream touchHLE.

## Features added in this fork

- **Game compatibility:** Additional APIs and fixes for P2G, Vice City, Inotia 1 / 2, Resident Evil Mercenaries VS., Dead Rising Mobile, and other games.
- **Android launcher:** Import games, long-press to manage them, view and share logs, and configure device profiles, game network access, and update checks.
- **Save transfers:** Import and export saves as ZIP files, with existing saves backed up before import.
- **Device profiles:** Automatically select a profile based on screen aspect ratio, or choose a global or per-game profile manually.
- **Controller support:** Implements the GameController APIs used by games, with P2G and Vice City tested on real hardware. Touch mappings and a virtual cursor remain available for other games.
- **Expanded P2G view:** Optionally expand the horizontal field of view while keeping touch controls in a central 16:9 area. In the default layout, joystick drags can continue into the letterbox margins.
- **Audio and lifecycle fixes:** Includes P2G background music looping, fixes for some Vice City sound artifacts, and improvements to quitting and Android background/resume handling.

See the [release notes](https://github.com/shawnw67/Podroid/releases) for version-specific changes. [CHANGELOG.md](CHANGELOG.md) currently preserves upstream history rather than a complete changelog for this fork.

## Compatibility

Podroid runs a game's ARM code and implements the iOS APIs it needs. It does not boot a complete iOS installation.

Only **32-bit ARM (armv6 / armv7) apps** are supported. Upstream compatibility focuses on the iPhone OS 2–4 era; this fork adds support for selected iOS 5 / 6-era games. That does not mean all apps targeting those OS versions will work.

Choosing an iPhone 5s, iPhone 6, or another device profile changes the reported device information, screen dimensions, and rendering configuration. **It does not enable arm64 emulation or upgrade the emulated OS to that device's iOS version.** Widescreen layouts and high-resolution assets depend on the specific IPA version and the game's implementation.

| Games receiving particular attention | Work in this fork |
| --- | --- |
| Monster Hunter Portable 2nd G (P2G) | Retina rendering, character name entry, save-related compatibility, native controllers, optional expanded view, and music looping |
| Grand Theft Auto: Vice City | OpenGL ES 2.0, version 1.4 compatibility, resolution handling, audio, native controllers, and quitting |
| Inotia 1 / 2 | Compatibility fixes needed to run the games |
| Resident Evil Mercenaries VS. | Compatibility, character rendering, and quitting fixes |
| Dead Rising Mobile | Compatibility and rendering fixes |

Different versions, localizations, or modified packages of the same game may behave differently. This table does not imply that every game mode, online feature, or device has been verified. The [upstream compatibility database](https://appdb.touchhle.org/) is a useful reference, but does not fully reflect this fork's additions.

## Download and installation

Download the Android ARM64 APK from this repository's [Releases](https://github.com/shawnw67/Podroid/releases). New release files use the name `Podroid_vX.Y.Z_Android_AArch64.apk`; older releases retain the UFtouchHLE name.

Only Android APKs are currently released. macOS, Windows, and Linux users can build from source.

Games are not included. Supply your own legally obtained, decrypted apps suitable for emulation. Release packages do not contain game IPAs.

### Android

The launcher currently uses Chinese labels. The English descriptions below explain the relevant buttons:

1. Install the APK and open the app.
2. Tap **导入游戏** (Import game) and select an `.ipa` file.
3. Tap a game to launch it, or long-press it to open its management menu.
4. Use the gear icon for global settings, or long-press a game to choose its device profile.

The Android package ID is `org.podroid.android`. This is a separate installation from the old `org.touchhleuf.android` app. Before removing the old app, export each game save to a shared location, then import the games and saves into Podroid. Settings must be configured again. Its data directory is:

```text
/storage/emulated/0/Android/data/org.podroid.android/files/
```

Common subdirectories:

```text
touchHLE_apps/          Game files
touchHLE_sandbox/       Game sandboxes and saves
touchHLE_save_exports/  Saves exported to the app's own directory
touchHLE_save_backups/  Existing saves backed up before import
```

Recent Android versions may restrict file manager access to `Android/data`. Prefer the launcher's import/export functions. UF uses a different package ID from upstream, so saves need to be transferred separately.

**允许游戏联网** (Allow game network access) controls networking for the emulated game. **自动检查更新** (Automatically check for updates) is a separate launcher setting. When an update is found, you can open its download page.

### Windows / macOS

Open the app from the release package. Place `.ipa` files or `.app` bundles in `touchHLE_apps` to run them through the built-in app picker. On Windows, extract the entire ZIP and retain the bundled fonts, libraries, and configuration files.

You can also supply a game path from a terminal:

```powershell
# Windows
.\touchHLE.exe "path\to\game.ipa"
```

```sh
# macOS: run from the directory containing the executable and its resources
./touchHLE "path/to/game.ipa"
```

See [OPTIONS_HELP.txt](OPTIONS_HELP.txt) for options and the [build documentation](dev-docs/building.md) for build and directory details.

## Transferring saves

On Android, long-press a game and select **导出存档** (Export saves) to create a ZIP. Import the same game on the destination device, long-press it, select **导入存档** (Import saves), and choose that ZIP.

Existing saves are moved to `touchHLE_save_backups` before import. Use the same game version and Bundle ID where possible; saves from different regions or versions may not be compatible. Save ZIPs do not contain the game itself.

On desktop, game data is stored in `touchHLE_sandbox`. Exit the game and back up the original directory before transferring files manually.

## Device profiles and display settings

Android settings offer **自动** (Automatic) selection or a fixed global profile. A per-game override is available by long-pressing a game. Available profiles include:

| Device | Landscape pixel dimensions |
| --- | --- |
| iPhone 3GS | 480 × 320 |
| iPhone 4 / 4S | 960 × 640 |
| iPhone 5 / 5s | 1136 × 640 |
| iPhone 6 | 1334 × 750 |
| iPhone 6 Plus | 2208 × 1242 |
| iPad 2 | 1024 × 768 |
| iPad Retina | 2048 × 1536 |

These are emulated screen configurations. Actual rendering dimensions may differ, and games that do not support a particular screen use compatible dimensions. Higher resolutions increase rendering cost and cannot add textures that are absent from the IPA.

Game defaults in `touchHLE_default_options.txt` and custom arguments in `touchHLE_options.txt` still apply. The launcher's device profile selection also contributes to the final configuration. Check the `Device:` line in the startup log to confirm the selected device.

For P2G, long-press the game and open **画面与按键布局** (Display and control layout) to select the expanded view. It increases the field of view on wider screens while keeping the touch control area centered. Return to the default layout if you encounter display or input problems.

## Controllers

Connect a controller to the host device through Bluetooth or USB before launching a game. Native input has been tested in P2G and Vice City with a physical controller. Other wired or Bluetooth controllers depend on the OS and SDL recognizing their buttons and axes correctly.

### P2G: Xbox / standard layout

| Input | Action |
| --- | --- |
| Left stick | Move |
| Right stick | Camera |
| A | Evade (×) |
| B | Attack / interact; gather or carve with your weapon sheathed (○) |
| X | Sheathe weapon / use item (□) |
| Y | Attack (△) |
| LB | L: recenter camera and combination actions |
| RB | R: sprint and weapon-specific actions |
| LT | Select |
| RT | Start |

Pushing the left stick fully gives normal movement; hold RB to sprint. Actions depend on the equipped weapon and current state. You usually need to sheathe with X before gathering or carving.

### Vice City

The game uses its native controller input path: the left stick controls movement and the right stick controls the camera. Other actions depend on whether you are on foot, driving, or in another state; follow the game's prompts. D-pad Down + B opens the map, and D-pad Up + B changes the camera view.

For other games without native controller input, touch mappings, a virtual cursor, and simulated tilt controls are available. See [OPTIONS_HELP.txt](OPTIONS_HELP.txt).

## Reporting problems

Open an issue in [this repository](https://github.com/shawnw67/Podroid/issues) and include:

- Podroid version, device model, and OS version.
- Game name, version, Bundle ID, device profile, and display layout.
- Steps to reproduce and `touchHLE_log.txt`; include `touchHLE_log_previous.txt` if relevant.
- Screenshots for display issues, or the controller model and Bluetooth/USB connection type for input issues.

The Android launcher can display and share logs. Vice City also has performance and audio comparison tools in its long-press menu.

## Building and development

See [dev-docs/building.md](dev-docs/building.md) for dependencies and complete instructions. Clone this fork with:

```sh
git clone --recurse-submodules https://github.com/shawnw67/Podroid.git Podroid
cd Podroid
```

Build and test locally on macOS:

```sh
export CMAKE_POLICY_VERSION_MINIMUM=3.5
cargo build --release
cargo test --release --lib
python3 dev-scripts/macos-integration-tests.py
```

Once the Android SDK, NDK, Rust target, and Gradle are configured, use the signing script:

```sh
dev-scripts/build-android-uf.sh /path/to/your.keystore
```

The script prompts for the signing password interactively. Use your own key. Updating an existing installation requires the same package ID and signing certificate.

Only Android APKs are currently released. Build and sign locally, upload to a release draft, then review it and add English release notes before publishing.

This fork includes AI-assisted code. Upstream touchHLE does not accept AI-generated code, so do not submit this fork's code upstream as a contribution. Keep fork-specific changes and discussion in this repository.

## License and acknowledgments

touchHLE © 2023–2026 touchHLE project contributors. This fork retains upstream and dependency copyright and license notices.

touchHLE's own source code is licensed under the **Mozilla Public License 2.0**. Due to dependency license compatibility requirements, distributed binaries are licensed under the **GNU GPL v3 or later**. Bundled libraries and fonts have their own terms; see [touchHLE_dylibs](touchHLE_dylibs/) and [touchHLE_fonts](touchHLE_fonts/). Dependency copyright information is available in the app's settings or through `--copyright`.

Thanks to [upstream touchHLE and its contributors](https://github.com/touchHLE/touchHLE/graphs/contributors), dynarmic, SDL, OpenAL Soft, Symphonia, the Rust ecosystem, all other open-source dependencies, and everyone who helps test the fork and report issues.

This project is not affiliated with or endorsed by Apple Inc. iPhone, iOS, iPod touch, and iPad are trademarks of their respective owners. Only run software you have obtained legally.
