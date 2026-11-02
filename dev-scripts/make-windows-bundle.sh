#!/bin/sh
set -e

# Bundles the touchHLE executable with the basic set of files needed for
# touchHLE to run (the same ones found in the macOS .app bundle or Android APK).
# This does not prepare a full release.

if [[ $# == 1 ]]; then
    PATH_TO_BINARY="$1"
    shift

    rm -rf Podroid_windows_bundle
    mkdir Podroid_windows_bundle
    cp $PATH_TO_BINARY Podroid_windows_bundle/
    cp -r ../Podroid_dylibs Podroid_windows_bundle/
    cp -r ../Podroid_fonts Podroid_windows_bundle/
    cp -r ../Podroid_default_options.txt Podroid_windows_bundle/
else
    echo "Incorrect usage."
    exit 1
fi
