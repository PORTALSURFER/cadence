#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
verifier="$script_dir/verify_macos_architecture.sh"
test_dir="$(mktemp -d -t cadence-architecture-test.XXXXXX)"
trap 'rm -rf "$test_dir"' EXIT

fake_bin="$test_dir/bin"
fake_executable="$test_dir/Cadence"
mkdir -p "$fake_bin"
printf '%s\n' 'fixture executable' > "$fake_executable"
chmod +x "$fake_executable"

printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' '[[ "$#" == 2 && "$1" == "-archs" ]]' 'printf "%s\n" "${FAKE_LIPO_ARCHS:?}"' > "$fake_bin/lipo"
chmod +x "$fake_bin/lipo"

PATH="$fake_bin:$PATH" FAKE_LIPO_ARCHS=arm64 "$verifier" "$fake_executable"

assert_rejected() {
    local architectures="$1"
    local output
    if output="$(PATH="$fake_bin:$PATH" FAKE_LIPO_ARCHS="$architectures" "$verifier" "$fake_executable" 2>&1)"; then
        printf 'architecture verifier accepted %s\n' "$architectures" >&2
        exit 1
    fi
    [[ "$output" == *"release executable must be exactly arm64"* ]]
}

assert_rejected x86_64
assert_rejected "arm64 x86_64"

echo "macOS architecture verification tests passed"
