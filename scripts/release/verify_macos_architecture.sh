#!/usr/bin/env bash
set -euo pipefail

if (($# != 1)); then
    echo "usage: verify_macos_architecture.sh EXECUTABLE" >&2
    exit 2
fi

executable_path="$1"
if [[ ! -f "$executable_path" || ! -r "$executable_path" || ! -x "$executable_path" ]]; then
    echo "bundled executable is missing, unreadable, or not executable: $executable_path" >&2
    exit 1
fi
if ! command -v lipo >/dev/null 2>&1; then
    echo "macOS lipo is required to verify the release executable architecture" >&2
    exit 1
fi

if ! architectures="$(lipo -archs "$executable_path")"; then
    echo "could not inspect the release executable architecture: $executable_path" >&2
    exit 1
fi
if [[ "$architectures" != "arm64" ]]; then
    echo "release executable must be exactly arm64; lipo reported: ${architectures:-<none>}" >&2
    exit 1
fi

echo "Verified arm64 release executable: $executable_path"
