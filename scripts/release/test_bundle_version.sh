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

assert_default_bundle_versions() {
    local output_path="$test_dir/Cadence-default.app"
    local expected_version
    local expected_short_version
    local expected_build_number

    expected_version="$(cargo pkgid --quiet --locked --manifest-path "$project_dir/Cargo.toml" --package cadence-native)"
    expected_version="${expected_version##*@}"
    expected_short_version="${expected_version%%-*}"
    expected_build_number="$expected_short_version"
    if [[ "$expected_version" == *-* ]]; then
        expected_build_number="${expected_version##*.}"
    fi

    "$bundle_script" \
        --executable "$fake_executable" \
        --output "$output_path"

    local actual_short_version
    local actual_build_number
    actual_short_version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$output_path/Contents/Info.plist")"
    actual_build_number="$(/usr/bin/plutil -extract CFBundleVersion raw -o - "$output_path/Contents/Info.plist")"
    [[ "$actual_short_version" == "$expected_short_version" ]]
    [[ "$actual_build_number" == "$expected_build_number" ]]
}

assert_rejected_without_explicit_version() {
    local option_name="$1"
    local option_value="$2"
    local output_name="$3"
    local output_path="$test_dir/Cadence-${output_name}.app"
    if "$bundle_script" \
        --executable "$fake_executable" \
        --output "$output_path" \
        "$option_name" "$option_value"; then
        printf '%s\n' "$option_name unexpectedly succeeded without --version" >&2
        exit 1
    fi
}

assert_rejected_empty_version() {
    local output_path="$test_dir/Cadence-empty-version.app"
    if "$bundle_script" \
        --executable "$fake_executable" \
        --output "$output_path" \
        --version ""; then
        printf '%s\n' "empty --version unexpectedly succeeded" >&2
        exit 1
    fi
}

assert_explicit_blank_overrides_derive() {
    local output_path="$test_dir/Cadence-explicit-blank-overrides.app"

    "$bundle_script" \
        --executable "$fake_executable" \
        --output "$output_path" \
        --version "0.1.0-rc.2" \
        --bundle-short-version "" \
        --bundle-build-number ""

    local actual_short_version
    local actual_build_number
    actual_short_version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$output_path/Contents/Info.plist")"
    actual_build_number="$(/usr/bin/plutil -extract CFBundleVersion raw -o - "$output_path/Contents/Info.plist")"
    [[ "$actual_short_version" == "0.1.0" ]]
    [[ "$actual_build_number" == "2" ]]
}

assert_default_bundle_versions
assert_rejected_without_explicit_version --bundle-short-version "0.1.0" nonempty-short-override-without-version
assert_rejected_without_explicit_version --bundle-short-version "" blank-short-override-without-version
assert_rejected_without_explicit_version --bundle-build-number "1" nonempty-build-override-without-version
assert_rejected_without_explicit_version --bundle-build-number "" blank-build-override-without-version
assert_rejected_empty_version
assert_explicit_blank_overrides_derive
assert_bundle_versions "0.1.0" "0.1.0" "0.1.0"
assert_bundle_versions "0.1.0-rc.2" "0.1.0" "2"
assert_bundle_versions "0.1.0-nightly.1" "0.1.0" "1"
assert_bundle_versions "0.1.1-nightly.6" "0.1.1" "6"
