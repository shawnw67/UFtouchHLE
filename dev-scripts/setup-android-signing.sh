#!/bin/sh
set -e

# One-time setup for signing touchHLE UF Android builds with a fixed key, so
# new APKs can be installed over old ones.
#
# 1. Creates a keystore outside the repository (you choose the password).
# 2. Stores it and its password as GitHub Actions secrets of the repository,
#    which the "Build for Android" workflow step uses.
#
# Keep a backup of the keystore and its password! Without them, future builds
# can't be installed over existing installs.
#
# Usage: dev-scripts/setup-android-signing.sh [owner/repo]

REPO="${1:-shawnw67/Podroid}"
KEYSTORE_DIR="$HOME/Documents/touchHLE-signing"
KEYSTORE="$KEYSTORE_DIR/Podroid_UF.keystore"
KEYTOOL="${KEYTOOL:-/opt/homebrew/opt/openjdk@17/bin/keytool}"

if [ -e "$KEYSTORE" ]; then
    echo "$KEYSTORE already exists, using it (delete it first to make a new key)."
else
    mkdir -p "$KEYSTORE_DIR"
    echo "Creating $KEYSTORE"
    echo "Choose a password (at least 6 characters) when asked, and remember it."
    "$KEYTOOL" -genkeypair -keystore "$KEYSTORE" -storetype PKCS12 \
        -alias touchhle-uf -keyalg RSA -keysize 4096 -validity 36500 \
        -dname "CN=touchHLE UF"
fi

echo
echo "Uploading the keystore to the secrets of $REPO ..."
base64 -i "$KEYSTORE" | gh secret set TOUCHHLE_KEYSTORE_BASE64 -R "$REPO"

echo
echo "Now paste the keystore password (input is hidden):"
gh secret set TOUCHHLE_KEYSTORE_PASSWORD -R "$REPO"

echo
gh secret list -R "$REPO"
echo
echo "Done. Back up $KEYSTORE and its password somewhere safe."
