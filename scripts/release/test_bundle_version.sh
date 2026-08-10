#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
bundle_script="$project_dir/scripts/build_native_app_bundle.sh"
test_dir="$(mktemp -d -t cadence-bundle-version-test.XXXXXX)"
trap 'rm -rf "$test_dir"' EXIT

fake_executable="$test_dir/cadence-native"
cp /bin/echo "$fake_executable"

assert_bundle_versions() {
    local release_version="$1"
    local expected_short_version="$2"
    local expected_build_number="$3"
    local output_path="$test_dir/Cadence-${expected_build_number}.app"

    "$bundle_script" \
        --executable "$fake_executable" \
        --output "$output_path" \
        --version "$release_version"

    local actual_short_version
    local actual_build_number
    actual_short_version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$output_path/Contents/Info.plist")"
    actual_build_number="$(/usr/bin/plutil -extract CFBundleVersion raw -o - "$output_path/Contents/Info.plist")"
    [[ "$actual_short_version" == "$expected_short_version" ]]
    [[ "$actual_build_number" == "$expected_build_number" ]]
}

assert_bundle_versions "0.1.0" "0.1.0" "0.1.0"
assert_bundle_versions "0.1.0-rc.2" "0.1.0" "2"
assert_bundle_versions "0.1.0-nightly.1" "0.1.0" "1"
