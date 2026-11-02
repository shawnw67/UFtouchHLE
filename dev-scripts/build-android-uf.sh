#!/bin/bash
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# Builds a touchHLE UF APK locally, branded and signed the same way as the
# GitHub Actions release build (see .github/workflows/Podroid_release.yml),
# so it can be installed over a release from GitHub.
#
# Usage: dev-scripts/build-android-uf.sh [KEYSTORE]
#
# KEYSTORE defaults to the one made by dev-scripts/setup-android-signing.sh.
# The keystore password is asked for and only passed to Gradle. Set GRADLE to
# the gradle executable if it isn't on the PATH. The Android SDK, NDK and Rust
# target need to be set up as for any Android build (see BUILDING.md).

set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEYSTORE="${1:-$HOME/Documents/touchHLE-signing/Podroid_UF.keystore}"
GRADLE="${GRADLE:-gradle}"

if [ ! -f "$KEYSTORE" ]; then
    echo "No keystore at $KEYSTORE" >&2
    exit 1
fi

read -r -s -p "Keystore password (input is hidden): " TOUCHHLE_KEYSTORE_PASSWORD
echo
export TOUCHHLE_KEYSTORE_PASSWORD
export TOUCHHLE_KEYSTORE_FILE="$KEYSTORE"
export TOUCHHLE_BRANDING=UF

cd "$ROOT/android"
# Make Gradle merge the native libraries again: it has been seen packaging the
# Rust library from a previous build.
rm -f app/src/main/jniLibs/arm64-v8a/libtouchHLE.so
rm -rf app/build/intermediates/merged_jni_libs app/build/intermediates/merged_native_libs app/build/intermediates/stripped_native_libs
"$GRADLE" assembleRelease -q

VERSION="$(cd "$ROOT" && cargo run -q --package Podroid_version)"
# The launcher's update check looks for this name (see checkForUpdate).
OUT="$ROOT/Podroid_${VERSION}_Android_AArch64.apk"
cp app/build/outputs/apk/release/app-release.apk "$OUT"
echo
echo "Built $OUT"
