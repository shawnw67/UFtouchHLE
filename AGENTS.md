# Podroid: notes for coding agents

This is **Podroid**, an unofficial fork of [touchHLE](https://github.com/touchHLE/touchHLE) (a high-level emulator for iPhone OS apps) with extra fixes for specific games, such as Inotia 1 and 2. Changes in this fork are AI-assisted. Upstream touchHLE does not accept AI-generated code (see `CONTRIBUTING.md`), so never submit anything from this fork upstream.

## Scope

- touchHLE emulates **32-bit ARM** (armv6/armv7) apps from the iPhone OS 2–4 era. Guest pointers are 32-bit throughout (`src/mem.rs`), and the CPU backend is dynarmic's A32 JIT. 64-bit-only (arm64) apps can't run.
- Game-specific workarounds should check that they are running the expected code (e.g. compare a function's start bytes) before patching or calling into the game.
- Don't commit app files (`Podroid_apps/*.ipa` or `Podroid_apps/*.ipa`) or bundle them into builds.

## Building and testing

- Test changes on a real Android phone before committing. macOS testing is not required.
- Before pushing: run `dev-scripts/lint.sh` and `dev-scripts/format.sh --check`. Formatting of C/Objective-C test code must satisfy clang-format (macOS 23, older on Linux/Windows), so keep both in mind.
- Android: `dev-scripts/build-android-uf.sh` builds a signed APK (package `org.podroid.android`). The person running it enters the keystore password; never store or pass secrets yourself.
- The Android emulator's OpenGL ES 1.1 translation has bugs that real phones don't.

## Releases

Only the Android APK is shipped; there is no CI package build anymore (the
build workflow was removed). Release flow, all local:

1. Commit and push the approved changes.
2. Push the tag `vX.Y.Z`.
3. Build the APK locally: `dev-scripts/build-android-uf.sh` (enters keystore password itself).
4. Upload it to the draft release: `GH_REPO=shawnw67/Podroid dev-scripts/upload-release-draft.sh vX.Y.Z /path/to/Podroid_vX.Y.Z_Android_AArch64.apk`.

Existing draft notes are preserved; published releases are not modified.
Review the draft and add English release notes before publishing.
