#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Upload a locally built Android APK to a draft release; never publish it or
# replace assets belonging to an already published release. Uses the GitHub
# REST API directly (no gh CLI required); the token comes from $GH_TOKEN or
# from git's stored credentials for github.com.
#
# Usage: GH_REPO=owner/repo dev-scripts/upload-release-draft.sh TAG APK_PATH
set -euo pipefail

tag="${1:?Usage: upload-release-draft.sh TAG APK_PATH}"
apk="${2:?Usage: upload-release-draft.sh TAG APK_PATH}"
: "${GH_REPO:?GH_REPO must name the destination repository}"

if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.-]+)?$ ]]; then
    echo "Not a supported version tag: $tag" >&2
    exit 1
fi
if [[ ! -s "$apk" ]]; then
    echo "Missing or empty APK: $apk" >&2
    exit 1
fi

if [[ -n "${GH_TOKEN:-}" ]]; then
    token="$GH_TOKEN"
else
    token="$(git credential fill <<< $'protocol=https\nhost=github.com' 2>/dev/null | sed -n 's/^password=//p')"
fi
if [[ -z "${token:-}" ]]; then
    echo "No GitHub token found: set GH_TOKEN or authenticate git with GitHub." >&2
    exit 1
fi

api="https://api.github.com/repos/$GH_REPO"
asset_name="$(basename "$apk")"

# Find the release for this tag (the tag endpoint doesn't return drafts, so
# list the releases instead); create a draft if there is none yet.
release_json="$(curl -sS -H "Authorization: Bearer $token" "$api/releases?per_page=100" | python3 -c '
import json, sys
for release in json.load(sys.stdin):
    if release.get("tag_name") == sys.argv[1]:
        print(json.dumps(release))
        break
' "$tag")"
if [[ -z "$release_json" ]]; then
    body="$(python3 -c 'import json,sys; print(json.dumps({"tag_name": sys.argv[1], "name": "Podroid " + sys.argv[1], "draft": True, "body": "Build artifacts for " + sys.argv[1] + ". Release notes are pending review."}))' "$tag")"
    release_json="$(curl -sS -X POST -H "Authorization: Bearer $token" \
        -H "Accept: application/vnd.github+json" -d "$body" "$api/releases")"
fi

if [[ "$(echo "$release_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("draft"))')" != "True" ]]; then
    echo "Release $tag is already published, or couldn't be created; refusing to change its assets." >&2
    echo "$release_json" >&2
    exit 1
fi

release_id="$(echo "$release_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"

# A failed upload can be retried: remove an existing asset with the same name.
existing_id="$(curl -sS -H "Authorization: Bearer $token" "$api/releases/$release_id/assets" | python3 -c "
import json,sys
name = sys.argv[1]
for a in json.load(sys.stdin):
    if a['name'] == name:
        print(a['id'])
" "$asset_name")"
if [[ -n "$existing_id" ]]; then
    curl -sS -X DELETE -H "Authorization: Bearer $token" "$api/releases/assets/$existing_id" >/dev/null
fi

uploaded="$(curl -sS -X POST -H "Authorization: Bearer $token" \
    -H "Content-Type: application/vnd.android.package-archive" \
    --data-binary "@$apk" \
    "https://uploads.github.com/repos/$GH_REPO/releases/$release_id/assets?name=$asset_name")"
echo "Uploaded: $(echo "$uploaded" | python3 -c 'import json,sys; print(json.load(sys.stdin)["name"])')"
echo "Draft release ready: https://github.com/$GH_REPO/releases/tag/$tag"
