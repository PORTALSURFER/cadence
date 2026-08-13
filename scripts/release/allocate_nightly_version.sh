#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf 'Usage: %s CURRENT_CARGO_BASE_VERSION\n' "$0" >&2
    exit 2
}

if (($# != 1)); then
    usage
fi

cargo_version="$1"
base_version_re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
if [[ ! "$cargo_version" =~ $base_version_re ]]; then
    printf 'Cargo base version must be numeric X.Y.Z: %s\n' "$cargo_version" >&2
    exit 1
fi

cargo_major="${BASH_REMATCH[1]}"
cargo_minor="${BASH_REMATCH[2]}"
cargo_patch="${BASH_REMATCH[3]}"

history_seen=false
history_major=0
history_minor=0
history_patch=0

is_newer_version() {
    local major="$1"
    local minor="$2"
    local patch="$3"

    if ((10#$major > 10#$history_major)); then
        return 0
    elif ((10#$major < 10#$history_major)); then
        return 1
    fi
    if ((10#$minor > 10#$history_minor)); then
        return 0
    elif ((10#$minor < 10#$history_minor)); then
        return 1
    fi
    ((10#$patch > 10#$history_patch))
}

while IFS= read -r release_name || [[ -n "$release_name" ]]; do
    [[ -n "$release_name" ]] || {
        printf 'Published release history contains an empty name.\n' >&2
        exit 1
    }

    release_version=""
    if [[ "$release_name" =~ ^Cadence[[:space:]]([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
        release_version="${BASH_REMATCH[1]}"
    elif [[ "$release_name" =~ ^Cadence[[:space:]]([0-9]+\.[0-9]+\.[0-9]+)-rc\.[1-9][0-9]*[[:space:]]rc$ ]]; then
        release_version="${BASH_REMATCH[1]}"
    elif [[ "$release_name" =~ ^Cadence[[:space:]]([0-9]+\.[0-9]+\.[0-9]+)-nightly\.[1-9][0-9]*[[:space:]]nightly$ ]]; then
        release_version="${BASH_REMATCH[1]}"
    else
        printf 'Malformed published Cadence release name: %s\n' "$release_name" >&2
        exit 1
    fi

    if [[ ! "$release_version" =~ $base_version_re ]]; then
        printf 'Malformed published Cadence release version: %s\n' "$release_version" >&2
        exit 1
    fi

    release_major="${BASH_REMATCH[1]}"
    release_minor="${BASH_REMATCH[2]}"
    release_patch="${BASH_REMATCH[3]}"
    if [[ "$history_seen" != true ]] || is_newer_version "$release_major" "$release_minor" "$release_patch"; then
        history_seen=true
        history_major="$release_major"
        history_minor="$release_minor"
        history_patch="$release_patch"
    fi
done

# An empty public history is the bootstrap case: the checked-out Cargo version
# is the only known floor, so the first reservation advances its patch once.
if [[ "$history_seen" != true ]]; then
    history_major="$cargo_major"
    history_minor="$cargo_minor"
    history_patch="$cargo_patch"
fi

if ((10#$cargo_major < 10#$history_major)) || \
    ((10#$cargo_major == 10#$history_major && 10#$cargo_minor < 10#$history_minor)); then
    printf 'Cargo base version %s is behind published release history %s.%s.%s.\n' \
        "$cargo_version" "$history_major" "$history_minor" "$history_patch" >&2
    exit 1
fi

if ((10#$cargo_major != 10#$history_major || 10#$cargo_minor != 10#$history_minor)); then
    printf 'Cargo base version %s is not in the published patch-version stream ending at %s.%s.%s.\n' \
        "$cargo_version" "$history_major" "$history_minor" "$history_patch" >&2
    exit 1
fi

patch_delta=$((10#$cargo_patch - 10#$history_patch))
if ((patch_delta < 0)); then
    printf 'Cargo base version %s is behind published release history %s.%s.%s.\n' \
        "$cargo_version" "$history_major" "$history_minor" "$history_patch" >&2
    exit 1
elif ((patch_delta == 0)); then
    next_patch=$((10#$history_patch + 1))
elif ((patch_delta == 1)); then
    # The package version was already reserved by an interrupted run.
    next_patch=$((10#$cargo_patch))
else
    printf 'Cargo base version %s is %s patch steps from published history %s.%s.%s.\n' \
        "$cargo_version" "$patch_delta" "$history_major" "$history_minor" "$history_patch" >&2
    exit 1
fi

printf '%s.%s.%s\n' "$cargo_major" "$cargo_minor" "$next_patch"
