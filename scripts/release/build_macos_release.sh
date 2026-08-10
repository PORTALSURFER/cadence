#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
output_dir="$project_dir/release-output"
version="${CADENCE_RELEASE_VERSION:-}"
channel="${CADENCE_RELEASE_CHANNEL:-stable}"
build_id="${CADENCE_RELEASE_BUILD_ID:-}"
source_git_sha="${GITHUB_SHA:-}"

usage() {
    cat <<'EOF'
Usage: build_macos_release.sh --version VERSION [--channel stable|rc|nightly] [--output-dir DIR] [--build-id ID]

Builds, signs, notarizes, staples, and describes the Cadence arm64 macOS release.
Production signing requires the Apple certificate and notary API-key environment
variables documented in README.md. The manifest Team ID is derived from the
selected Developer ID Application identity.
EOF
}

team_id_from_codesign_identity() {
    local identity="${1:-}"
    local identity_re='^Developer ID Application:.*\(([A-Z0-9]{10})\)$'
    if [[ "$identity" =~ $identity_re ]]; then
        printf '%s\n' "${BASH_REMATCH[1]}"
        return 0
    fi
    echo "selected Developer ID Application identity does not end with a valid ten-character Team ID in parentheses: $identity" >&2
    return 1
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
    return 0
fi

while (($# > 0)); do
    case "$1" in
        --version)
            [[ $# -ge 2 ]] || { echo "--version requires a value" >&2; exit 2; }
            version="$2"
            shift 2
            ;;
        --channel)
            [[ $# -ge 2 ]] || { echo "--channel requires a value" >&2; exit 2; }
            channel="$2"
            shift 2
            ;;
        --output-dir)
            [[ $# -ge 2 ]] || { echo "--output-dir requires a value" >&2; exit 2; }
            output_dir="$2"
            shift 2
            ;;
        --build-id)
            [[ $# -ge 2 ]] || { echo "--build-id requires a value" >&2; exit 2; }
            build_id="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "$channel" in
    stable|rc|nightly)
        ;;
    *)
        echo "invalid release channel: $channel (expected stable, rc, or nightly)" >&2
        exit 2
        ;;
esac

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Cadence production releases must be built on macOS." >&2
    exit 1
fi

base_version_re='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
case "$channel" in
    stable)
        version_re="^${base_version_re}$"
        ;;
    rc)
        version_re="^${base_version_re}-rc\.[1-9][0-9]*$"
        ;;
    nightly)
        version_re="^${base_version_re}-nightly\.[1-9][0-9]*$"
        ;;
esac
if [[ ! "$version" =~ $version_re ]]; then
    echo "$channel releases require the matching channel version format: $version" >&2
    exit 2
fi
if [[ -z "$source_git_sha" ]]; then
    source_git_sha="$(git -C "$project_dir" rev-parse HEAD)"
fi
source_git_sha="$(printf '%s' "$source_git_sha" | tr '[:upper:]' '[:lower:]')"
if [[ ! "$source_git_sha" =~ ^[0-9a-f]{40}$ ]]; then
    echo "GITHUB_SHA or HEAD must be a 40-character commit SHA." >&2
    exit 1
fi
if [[ -n "$(git -C "$project_dir" status --porcelain --untracked-files=all)" ]]; then
    echo "production release builds require a clean checkout" >&2
    exit 1
fi
if [[ -z "$build_id" ]]; then
    release_run_id="${GITHUB_RUN_NUMBER:-$(date -u +%Y%m%d%H%M%S)}"
    if [[ "$channel" == stable ]]; then
        build_id="cadence-v${version}-${release_run_id}"
    else
        build_id="cadence-${channel}-${release_run_id}-${source_git_sha:0:12}"
    fi
fi
if [[ ! "$build_id" =~ ^[a-z0-9][a-z0-9._-]{1,127}$ ]]; then
    echo "invalid release build id: $build_id" >&2
    exit 2
fi

require_env() {
    local name="$1"
    if [[ -z "${!name:-}" ]]; then
        echo "missing required production signing secret: $name" >&2
        exit 1
    fi
}

require_env APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64
require_env APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD
require_env APPLE_NOTARY_KEY_BASE64
require_env APPLE_NOTARY_KEY_ID
require_env APPLE_NOTARY_ISSUER_ID

output_dir="$(cd -- "$(dirname -- "$output_dir")" && pwd)/$(basename -- "$output_dir")"
work_dir="$(mktemp -d -t cadence-release.XXXXXX)"
keychain_path="$work_dir/cadence-release-signing.keychain-db"
keychain_password="$(/usr/bin/uuidgen | tr -d '-')"
certificate_path="$work_dir/developer-id-application.p12"
notary_key_path="$work_dir/AuthKey_${APPLE_NOTARY_KEY_ID}.p8"
notary_zip_path="$work_dir/cadence-notary.zip"
app_path="$work_dir/Cadence.app"
original_keychains_path="$work_dir/original-keychains.txt"
notary_response_path="$work_dir/notary-response.json"

decode_base64() {
    local value="$1"
    local output="$2"
    if printf '%s' "$value" | base64 --decode > "$output" 2>/dev/null; then return 0; fi
    if printf '%s' "$value" | base64 -D > "$output" 2>/dev/null; then return 0; fi
    echo "could not decode Apple signing material" >&2
    exit 1
}

cleanup() {
    if [[ -f "$original_keychains_path" ]]; then
        local original_keychains=()
        while IFS= read -r keychain; do
            [[ -n "$keychain" ]] && original_keychains+=("$keychain")
        done < "$original_keychains_path"
        if ((${#original_keychains[@]} > 0)); then
            security list-keychains -d user -s "${original_keychains[@]}" >/dev/null 2>&1 || true
        fi
    fi
    security delete-keychain "$keychain_path" >/dev/null 2>&1 || true
    rm -rf "$work_dir"
}
trap cleanup EXIT

decode_base64 "$APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64" "$certificate_path"
decode_base64 "$APPLE_NOTARY_KEY_BASE64" "$notary_key_path"
chmod 600 "$certificate_path" "$notary_key_path"
openssl pkcs12 -in "$certificate_path" -passin "pass:${APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD}" -info -noout >/dev/null 2>&1 || {
    echo "Apple certificate material is not a readable .p12 for the supplied password." >&2
    exit 1
}

security list-keychains -d user | sed 's/[[:space:]]*"//g; s/"$//' > "$original_keychains_path"
security create-keychain -p "$keychain_password" "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p "$keychain_password" "$keychain_path"
original_keychains=()
while IFS= read -r keychain; do
    [[ -n "$keychain" ]] && original_keychains+=("$keychain")
done < "$original_keychains_path"
security list-keychains -d user -s "$keychain_path" "${original_keychains[@]}"
security import "$certificate_path" -P "$APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD" -A -t cert -f pkcs12 -k "$keychain_path"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain_path" >/dev/null

codesign_identity="${APPLE_CODESIGN_IDENTITY:-}"
if [[ -z "$codesign_identity" ]]; then
    codesign_identity="$(security find-identity -v -p codesigning "$keychain_path" | sed -n 's/.*"\(Developer ID Application:.*\)".*/\1/p' | head -n 1)"
fi
if [[ -z "$codesign_identity" || "$codesign_identity" != Developer\ ID\ Application:* ]]; then
    echo "no Developer ID Application identity was found in the imported certificate" >&2
    exit 1
fi
team_id="$(team_id_from_codesign_identity "$codesign_identity")" || exit 1

bundle_short_version="${version%%-*}"
bundle_build_number="$bundle_short_version"
if [[ "$channel" != stable ]]; then
    bundle_build_number="${version##*.}"
fi

target_triple=aarch64-apple-darwin
executable_path="$project_dir/target/$target_triple/release/cadence-native"
cargo build --target "$target_triple" --release --locked --manifest-path "$project_dir/Cargo.toml"
"$project_dir/scripts/release/verify_macos_architecture.sh" "$executable_path"
rm -rf "$app_path"
"$project_dir/scripts/build_native_app_bundle.sh" \
    --executable "$executable_path" \
    --output "$app_path" \
    --version "$version" \
    --bundle-short-version "$bundle_short_version" \
    --bundle-build-number "$bundle_build_number" \
    --signing-identity "$codesign_identity"

"$project_dir/scripts/release/verify_macos_architecture.sh" "$app_path/Contents/MacOS/Cadence"
codesign --verify --deep --strict --verbose=2 "$app_path"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$notary_zip_path"
xcrun notarytool submit "$notary_zip_path" \
    --key "$notary_key_path" \
    --key-id "$APPLE_NOTARY_KEY_ID" \
    --issuer "$APPLE_NOTARY_ISSUER_ID" \
    --wait \
    --output-format json > "$notary_response_path"

notary_submission_id="$(python3 - "$notary_response_path" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    response = json.load(handle)
if response.get("status") != "Accepted":
    raise SystemExit(f"Apple notarization was not accepted: {response.get('status', 'unknown')}")
print(response.get("id", ""))
PY
)"
if [[ ! "$notary_submission_id" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$ ]]; then
    echo "Apple notarization did not return a valid submission id." >&2
    exit 1
fi

xcrun stapler staple "$app_path"
xcrun stapler validate "$app_path"
spctl --assess --type execute --verbose=2 "$app_path"

rm -rf "$output_dir"
mkdir -p "$output_dir"
artifact_name="cadence-v${version}-macos-arm64.zip"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$output_dir/$artifact_name"
cp "$project_dir/reference/cadence-ui-repainted.png" "$output_dir/cadence-default-ui-1594x987.png"
cp "$project_dir/CHANGELOG.md" "$output_dir/CHANGELOG.md"
(cd "$output_dir" && shasum -a 256 "$artifact_name" "cadence-default-ui-1594x987.png" "CHANGELOG.md" > SHA256SUMS.txt)

released_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
node "$project_dir/scripts/release/create_manifest.mjs" \
    --output-dir "$output_dir" \
    --version "$version" \
    --channel "$channel" \
    --build-id "$build_id" \
    --git-sha "$source_git_sha" \
    --released-at "$released_at" \
    --team-id "$team_id" \
    --notary-submission-id "$notary_submission_id"

echo "Built signed Cadence release in $output_dir"
