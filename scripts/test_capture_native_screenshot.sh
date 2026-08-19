#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
harness="$project_dir/scripts/capture_native_screenshot.sh"
test_dir="$(mktemp -d -t cadence-capture-screenshot-test.XXXXXX)"
fake_bin="$test_dir/bin"
mkdir -p "$fake_bin"
trap 'rm -rf "$test_dir"' EXIT

cat > "$fake_bin/uname" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' Darwin
EOF

cat > "$fake_bin/swift" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_SWIFT_LOG:?}"
printf '%s\n' "${FAKE_WINDOW_RECORDS:-}"
EOF

cat > "$fake_bin/osascript" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_OSASCRIPT_LOG:?}"
EOF

cat > "$fake_bin/screencapture" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "${FAKE_CAPTURE_ARGS:?}"
output_path=""
for argument in "$@"; do
    output_path="$argument"
done
printf 'fake png\n' > "$output_path"
EOF

cat > "$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_CARGO_LOG:?}"
exit 99
EOF

cat > "$fake_bin/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

chmod +x "$fake_bin"/*

swift_log="$test_dir/swift.log"
osascript_log="$test_dir/osascript.log"
capture_args="$test_dir/capture-args.log"
cargo_log="$test_dir/cargo.log"

run_capture() {
    local label="$1"
    local records="$2"
    local expected_window_id="$3"
    local expected_owner_pid="$4"
    local output_path="$5"

    : > "$swift_log"
    : > "$osascript_log"
    : > "$capture_args"
    : > "$cargo_log"
    rm -f "$output_path"

    if ! FAKE_WINDOW_RECORDS="$records" \
        FAKE_SWIFT_LOG="$swift_log" \
        FAKE_OSASCRIPT_LOG="$osascript_log" \
        FAKE_CAPTURE_ARGS="$capture_args" \
        FAKE_CARGO_LOG="$cargo_log" \
        PATH="$fake_bin:$PATH" \
        "$harness" --hover 150 210 --output "$output_path"; then
        printf '%s\n' "$label capture failed." >&2
        exit 1
    fi

    [[ -s "$output_path" ]] || {
        printf '%s\n' "$label did not create its output." >&2
        exit 1
    }
    [[ ! -s "$cargo_log" ]] || {
        printf '%s\n' "$label unexpectedly ran the cargo fallback." >&2
        exit 1
    }
    grep -F -- "unix id is $expected_owner_pid" "$osascript_log" >/dev/null || {
        printf '%s\n' "$label fronted the wrong owner PID." >&2
        exit 1
    }
    grep -Fx -- '-l' "$capture_args" >/dev/null || {
        printf '%s\n' "$label did not pass a window ID to screencapture." >&2
        exit 1
    }
    grep -Fx -- "$expected_window_id" "$capture_args" >/dev/null || {
        printf '%s\n' "$label selected the wrong window ID." >&2
        exit 1
    }
    tail -n 1 "$capture_args" | grep -Fx -- "$output_path" >/dev/null || {
        printf '%s\n' "$label passed the wrong output path." >&2
        exit 1
    }
}

bundled_records=$'17\t0\tCadence\tCadence — local track review\n18\t333\tOther\tCadence — local track review\n19\t444\tCadence\tother title\n20\t0\tCadence\tCadence — local track review\n21\t222\tCadence\tCadence — local track review'
run_capture \
    "bundled Cadence" \
    "$bundled_records" \
    21 \
    222 \
    "$test_dir/bundled capture.png"

raw_records=$'31\t555\tOther\tCadence — local track review\n32\t666\tcadence-native\tCadence — local track review\n33\t777\tcadence-native\tother title\n34\t0\tcadence-native\tCadence — local track review'
run_capture \
    "raw cadence-native" \
    "$raw_records" \
    32 \
    666 \
    "$test_dir/raw-cadence-native.png"

printf '%s\n' "capture_native_screenshot.sh tests passed"
