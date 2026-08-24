#!/usr/bin/env bash
set -euo pipefail

caller_cwd="$PWD"
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
output_dir="$project_dir/release-output"
version="${CADENCE_RELEASE_VERSION:-}"
channel="${CADENCE_RELEASE_CHANNEL:-stable}"
build_id="${CADENCE_RELEASE_BUILD_ID:-}"
source_git_sha=""
screenshot_source_git_sha=""
base_version_re='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'

usage() {
    cat <<'EOF'
Usage: build_macos_release.sh --version VERSION [--channel stable|rc|nightly] [--output-dir DIR] [--build-id ID]

Builds, signs, notarizes, staples, and describes the Cadence arm64 macOS release.
--output-dir DIR is resolved relative to the caller's current working directory.
Its parent must already exist, and DIR must be absent or an existing empty
directory. Symlinks, files, special nodes, nonempty directories, ambiguous or
root paths, and the repository root are rejected; caller-controlled directories
are never recursively removed.
Production signing requires the Apple certificate and notary API-key environment
variables documented in README.md. The manifest Team ID is derived from the
selected Developer ID Application identity.
EOF
}

validate_release_output_dir_path() {
    local requested="${1:-}"
    local caller_directory="${2:-$PWD}"
    local parent_directory
    local output_name

    if [[ -z "$requested" ]]; then
        echo "invalid release output directory: path must not be empty" >&2
        return 1
    fi

    while [[ "$requested" == */ && "$requested" != "/" ]]; do
        requested="${requested%/}"
    done
    case "$requested" in
        .|..|/)
            echo "invalid release output directory: ambiguous or root path: $requested" >&2
            return 1
            ;;
    esac

    if [[ "$requested" != /* ]]; then
        requested="$caller_directory/$requested"
    fi

    parent_directory="$(dirname "$requested")"
    output_name="$(basename "$requested")"
    case "$output_name" in
        ""|.|..)
            echo "invalid release output directory: ambiguous or root path: $requested" >&2
            return 1
            ;;
    esac

    if [[ ! -d "$parent_directory" ]]; then
        echo "invalid release output directory: parent does not already exist: $parent_directory" >&2
        return 1
    fi
    if ! parent_directory="$(cd "$parent_directory" && pwd -P)"; then
        echo "invalid release output directory: could not resolve parent: $parent_directory" >&2
        return 1
    fi
    printf '%s/%s\n' "$parent_directory" "$output_name"
}

validate_release_output_dir() {
    local requested="${1:-}"
    local caller_directory="${2:-$PWD}"
    local candidate
    local child

    candidate="$(validate_release_output_dir_path "$requested" "$caller_directory")" || return 1
    if [[ "$candidate" == "$project_dir" ]]; then
        echo "invalid release output directory: repository root is not allowed: $candidate" >&2
        return 1
    fi
    if [[ -L "$candidate" ]]; then
        echo "invalid release output directory: symlinks are not allowed: $candidate" >&2
        return 1
    fi
    if [[ ! -e "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return 0
    fi
    if [[ ! -d "$candidate" ]]; then
        echo "invalid release output directory: target must be a directory: $candidate" >&2
        return 1
    fi
    if [[ ! -r "$candidate" || ! -x "$candidate" ]]; then
        echo "invalid release output directory: target cannot be inspected: $candidate" >&2
        return 1
    fi
    for child in "$candidate"/* "$candidate"/.[!.]* "$candidate"/..?*; do
        if [[ -e "$child" || -L "$child" ]]; then
            echo "invalid release output directory: target must be empty: $candidate" >&2
            return 1
        fi
    done
    printf '%s\n' "$candidate"
}

prepare_release_output_dir() {
    local requested="${1:-}"
    local caller_directory="${2:-$PWD}"
    local candidate

    candidate="$(validate_release_output_dir "$requested" "$caller_directory")" || return 1
    if [[ ! -e "$candidate" && ! -L "$candidate" ]]; then
        if ! mkdir "$candidate"; then
            echo "could not create release output directory: $candidate" >&2
            return 1
        fi
    fi
    validate_release_output_dir "$candidate" "$caller_directory" >/dev/null || return 1
    printf '%s\n' "$candidate"
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

resolve_verified_source_sha() {
    local head_sha
    local provided_sha

    if ! head_sha="$(git -C "$project_dir" rev-parse --verify HEAD^{commit})"; then
        echo "could not resolve the checked-out repository HEAD." >&2
        return 1
    fi
    head_sha="$(printf '%s' "$head_sha" | tr '[:upper:]' '[:lower:]')"
    if [[ ! "$head_sha" =~ ^[0-9a-f]{40}$ ]]; then
        echo "checked-out repository HEAD is not a 40-character commit SHA." >&2
        return 1
    fi

    provided_sha="${GITHUB_SHA:-}"
    if [[ -n "$provided_sha" ]]; then
        provided_sha="$(printf '%s' "$provided_sha" | tr '[:upper:]' '[:lower:]')"
        if [[ ! "$provided_sha" =~ ^[0-9a-f]{40}$ ]]; then
            echo "GITHUB_SHA must be a 40-character commit SHA." >&2
            return 1
        fi
        if [[ "$provided_sha" != "$head_sha" ]]; then
            echo "GITHUB_SHA does not match the checked-out repository HEAD." >&2
            return 1
        fi
    fi

    printf '%s\n' "$head_sha"
}

verify_release_screenshot_provenance() {
    local release_source_sha="${1:-}"
    local screenshot_path="${2:-$project_dir/reference/cadence-ui-repainted.png}"
    local metadata_path="${3:-$project_dir/reference/cadence-ui-repainted.png.json}"
    local metadata_sha256
    local metadata_dimensions
    local metadata_source_git_sha
    local screenshot_sha256
    local screenshot_dimensions
    local screenshot_width
    local screenshot_height

    if [[ ! "$release_source_sha" =~ ^[0-9a-f]{40}$ ]]; then
        echo "release source SHA must be a 40-character lowercase commit SHA before screenshot verification." >&2
        return 1
    fi
    if [[ ! -f "$screenshot_path" || -L "$screenshot_path" ]]; then
        echo "release screenshot must be a checked-in regular file: $screenshot_path" >&2
        return 1
    fi
    if [[ ! -f "$metadata_path" || -L "$metadata_path" ]]; then
        echo "release screenshot metadata sidecar is missing: $metadata_path" >&2
        return 1
    fi

    if ! metadata_sha256="$(jq -er '
        if type == "object"
           and (.sha256 | type) == "string"
           and (.sha256 | test("^[0-9a-f]{64}$"))
        then .sha256
        else error("sha256 must be a 64-character lowercase hexadecimal string")
        end
    ' "$metadata_path")"; then
        echo "release screenshot metadata sidecar has invalid sha256: $metadata_path" >&2
        return 1
    fi
    if ! metadata_dimensions="$(jq -er '
        if type == "object" and .width == 1594 and .height == 987
        then "\(.width) \(.height)"
        else error("width and height must be 1594 and 987")
        end
    ' "$metadata_path")"; then
        echo "release screenshot metadata sidecar has invalid dimensions: $metadata_path" >&2
        return 1
    fi
    if ! metadata_source_git_sha="$(jq -er '
        if type == "object"
           and (.source_git_sha | type) == "string"
           and (.source_git_sha | test("^[0-9a-f]{40}$"))
        then .source_git_sha
        else error("source_git_sha must be a 40-character lowercase commit SHA")
        end
    ' "$metadata_path")"; then
        echo "release screenshot metadata sidecar has invalid source_git_sha: $metadata_path" >&2
        return 1
    fi

    if ! screenshot_sha256="$(shasum -a 256 "$screenshot_path" | awk '{print $1}')"; then
        echo "could not hash the release screenshot: $screenshot_path" >&2
        return 1
    fi
    if [[ "$screenshot_sha256" != "$metadata_sha256" ]]; then
        echo "release screenshot SHA-256 does not match its metadata sidecar." >&2
        return 1
    fi

    if ! screenshot_dimensions="$(
        sips -g pixelWidth -g pixelHeight "$screenshot_path" 2>/dev/null |
            awk '
                $1 == "pixelWidth:" { width = $2 }
                $1 == "pixelHeight:" { height = $2 }
                END {
                    if (width !~ /^[0-9]+$/ || height !~ /^[0-9]+$/) exit 1
                    print width, height
                }
            '
    )"; then
        echo "could not read release screenshot dimensions: $screenshot_path" >&2
        return 1
    fi
    read -r screenshot_width screenshot_height <<< "$screenshot_dimensions"
    if [[ "$screenshot_width $screenshot_height" != "$metadata_dimensions" ]]; then
        echo "release screenshot dimensions do not match its metadata sidecar." >&2
        return 1
    fi

    if ! git -C "$project_dir" merge-base --is-ancestor "$metadata_source_git_sha" "$release_source_sha" >/dev/null 2>&1; then
        echo "release screenshot source commit $metadata_source_git_sha is not an ancestor of release source SHA $release_source_sha." >&2
        return 1
    fi

    printf '%s\n' "$metadata_source_git_sha"
}

resolve_root_cargo_package_version() {
    local metadata
    local root_manifest="$project_dir/Cargo.toml"
    local package_version

    if ! metadata="$(env \
        -u APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64 \
        -u APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD \
        -u APPLE_NOTARY_KEY_BASE64 \
        -u APPLE_NOTARY_KEY_ID \
        -u APPLE_NOTARY_ISSUER_ID \
        -u APPLE_CODESIGN_IDENTITY \
        cargo metadata \
        --locked \
        --no-deps \
        --format-version 1 \
        --manifest-path "$root_manifest" \
        2>/dev/null
    )"; then
        echo "could not read the locked Cargo metadata for the root Cadence package." >&2
        return 1
    fi
    if ! package_version="$(jq -er \
        --arg root_manifest "$root_manifest" \
        '[.packages[]? | select(.name == "cadence-native" and .manifest_path == $root_manifest) | .version]
         | if length == 1 and .[0] != "" then .[0] else error("root cadence-native package is missing or ambiguous") end' \
        <<<"$metadata"
    )"; then
        echo "locked Cargo metadata must contain exactly one root cadence-native package." >&2
        return 1
    fi
    printf '%s\n' "$package_version"
}

resolve_locked_cargo_package_version() {
    local lockfile="$project_dir/Cargo.lock"
    local package_version

    if ! package_version="$(awk '
        function finish_package() {
            if (!is_root) {
                return
            }
            matches++
            if (version == "") {
                malformed=1
            } else if (matches == 1) {
                selected=version
            }
        }
        /^\[\[package\]\]$/ {
            finish_package()
            in_package=1
            is_root=0
            version=""
            next
        }
        in_package && /^\[/ {
            finish_package()
            in_package=0
            is_root=0
            version=""
        }
        in_package && $0 == "name = \"cadence-native\"" {
            is_root=1
            next
        }
        in_package && /^version = \"/ {
            version=$0
            sub(/^version = \"/, "", version)
            sub(/\"$/, "", version)
        }
        END {
            finish_package()
            if (matches != 1 || malformed) {
                exit 1
            }
            print selected
        }
    ' "$lockfile")"; then
        echo "Cargo.lock must contain exactly one root cadence-native package." >&2
        return 1
    fi
    printf '%s\n' "$package_version"
}

validate_release_version_against_cargo() {
    local package_version="$1"
    local lock_version="$2"
    local package_numeric_version
    local version_re

    [[ "$package_version" == "$lock_version" ]] || {
        echo "Cargo.toml and Cargo.lock cadence-native package versions differ: $package_version vs $lock_version" >&2
        return 1
    }
    package_numeric_version="${package_version%%-*}"
    [[ "$package_numeric_version" =~ ^${base_version_re}$ ]] || {
        echo "root cadence-native package version must have a numeric X.Y.Z base: $package_version" >&2
        return 1
    }

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
        *)
            echo "invalid release channel: $channel (expected stable, rc, or nightly)" >&2
            return 2
            ;;
    esac
    if [[ ! "$version" =~ $version_re ]]; then
        echo "$channel releases require the matching channel version format: $version" >&2
        return 2
    fi

    if [[ "$channel" == stable ]]; then
        [[ "$version" == "$package_version" ]] || {
            echo "stable release version $version does not match root cadence-native package version $package_version" >&2
            return 1
        }
    else
        [[ "${version%%-*}" == "$package_numeric_version" ]] || {
            echo "$channel release version $version does not match root cadence-native package base version $package_numeric_version" >&2
            return 1
        }
    fi
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

if ! source_git_sha="$(resolve_verified_source_sha)"; then
    exit 1
fi

if ! root_cargo_package_version="$(resolve_root_cargo_package_version)"; then
    exit 1
fi
if ! locked_cargo_package_version="$(resolve_locked_cargo_package_version)"; then
    exit 1
fi
validate_release_version_against_cargo "$root_cargo_package_version" "$locked_cargo_package_version"

if ! output_dir="$(validate_release_output_dir "$output_dir" "$caller_cwd")"; then
    exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Cadence production releases must be built on macOS." >&2
    exit 1
fi

if [[ -n "$(git -C "$project_dir" status --porcelain --untracked-files=all)" ]]; then
    echo "production release builds require a clean checkout" >&2
    exit 1
fi
if ! screenshot_source_git_sha="$(verify_release_screenshot_provenance "$source_git_sha")"; then
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

apple_developer_id_application_cert_base64="${APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64:-}"
apple_developer_id_application_cert_password="${APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD:-}"
apple_notary_key_base64="${APPLE_NOTARY_KEY_BASE64:-}"
apple_notary_key_id="${APPLE_NOTARY_KEY_ID:-}"
apple_notary_issuer_id="${APPLE_NOTARY_ISSUER_ID:-}"
codesign_identity_override="${APPLE_CODESIGN_IDENTITY:-}"

require_release_secret() {
    local value_name="$1"
    local environment_name="$2"
    [[ -n "${!value_name}" ]] || {
        echo "missing required production signing secret: $environment_name" >&2
        exit 1
    }
}

require_release_secret apple_developer_id_application_cert_base64 APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64
require_release_secret apple_developer_id_application_cert_password APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD
require_release_secret apple_notary_key_base64 APPLE_NOTARY_KEY_BASE64
require_release_secret apple_notary_key_id APPLE_NOTARY_KEY_ID
require_release_secret apple_notary_issuer_id APPLE_NOTARY_ISSUER_ID

unset \
    APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64 \
    APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD \
    APPLE_NOTARY_KEY_BASE64 \
    APPLE_NOTARY_KEY_ID \
    APPLE_NOTARY_ISSUER_ID \
    APPLE_CODESIGN_IDENTITY

bundle_short_version="${version%%-*}"
bundle_build_number="$bundle_short_version"
if [[ "$channel" != stable ]]; then
    bundle_build_number="${version##*.}"
fi

target_triple=aarch64-apple-darwin
executable_path="$project_dir/target/$target_triple/release/cadence-native"
env \
    -u APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64 \
    -u APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD \
    -u APPLE_NOTARY_KEY_BASE64 \
    -u APPLE_NOTARY_KEY_ID \
    -u APPLE_NOTARY_ISSUER_ID \
    -u APPLE_CODESIGN_IDENTITY \
    cargo build --target "$target_triple" --release --locked --manifest-path "$project_dir/Cargo.toml"
"$project_dir/scripts/release/verify_macos_architecture.sh" "$executable_path"

work_dir="$(mktemp -d -t cadence-release.XXXXXX)"
keychain_path="$work_dir/cadence-release-signing.keychain-db"
keychain_password="$(/usr/bin/uuidgen | tr -d '-')"
certificate_path="$work_dir/developer-id-application.p12"
notary_key_path="$work_dir/AuthKey_${apple_notary_key_id}.p8"
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

decode_base64 "$apple_developer_id_application_cert_base64" "$certificate_path"
decode_base64 "$apple_notary_key_base64" "$notary_key_path"
chmod 600 "$certificate_path" "$notary_key_path"
openssl pkcs12 -in "$certificate_path" -passin "pass:${apple_developer_id_application_cert_password}" -info -noout >/dev/null 2>&1 || {
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
security import "$certificate_path" -P "$apple_developer_id_application_cert_password" -A -t cert -f pkcs12 -k "$keychain_path"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain_path" >/dev/null

codesign_identity="$codesign_identity_override"
if [[ -z "$codesign_identity" ]]; then
    codesign_identity="$(security find-identity -v -p codesigning "$keychain_path" | sed -n 's/.*"\(Developer ID Application:.*\)".*/\1/p' | head -n 1)"
fi
if [[ -z "$codesign_identity" || "$codesign_identity" != Developer\ ID\ Application:* ]]; then
    echo "no Developer ID Application identity was found in the imported certificate" >&2
    exit 1
fi
team_id="$(team_id_from_codesign_identity "$codesign_identity")" || exit 1

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
    --key-id "$apple_notary_key_id" \
    --issuer "$apple_notary_issuer_id" \
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

if ! output_dir="$(prepare_release_output_dir "$output_dir" "$caller_cwd")"; then
    exit 1
fi
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
    --screenshot-source-git-sha "$screenshot_source_git_sha" \
    --released-at "$released_at" \
    --team-id "$team_id" \
    --notary-submission-id "$notary_submission_id"

echo "Built signed Cadence release in $output_dir"
