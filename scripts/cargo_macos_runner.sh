#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"

if (($# == 0)); then
    printf '%s\n' "Cargo macOS runner requires an executable path." >&2
    exit 2
fi

raw_executable="$1"
shift

if [[ "$raw_executable" == /* ]]; then
    executable_path="$raw_executable"
else
    executable_path="$PWD/$raw_executable"
fi
executable_path="$(cd -- "$(dirname -- "$executable_path")" && pwd -P)/$(basename -- "$executable_path")"

case "$executable_path" in
    */target/debug/cadence-native|*/target/release/cadence-native)
        ;;
    *)
        exec "$raw_executable" "$@"
        ;;
esac

if [[ "$(uname -s)" != "Darwin" ]]; then
    exec "$raw_executable" "$@"
fi

bundle_path="$project_dir/target/dev-app/Cadence.app"
"$project_dir/scripts/build_native_app_bundle.sh" \
    "$bundle_path" \
    --executable "$executable_path"

if (($# == 0)); then
    exec /usr/bin/open -n -W "$bundle_path"
else
    exec /usr/bin/open -n -W "$bundle_path" --args "$@"
fi
