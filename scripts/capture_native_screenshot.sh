#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    printf '%s\n' "Cadence native screenshots currently require macOS." >&2
    exit 1
fi

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
output_path="artifacts/screenshots/cadence-native.png"
hover_x=""
hover_y=""

while (($# > 0)); do
    case "$1" in
        --hover)
            if (($# < 3)); then
                printf '%s\n' "--hover requires an X and Y screen coordinate." >&2
                exit 2
            fi
            hover_x="$2"
            hover_y="$3"
            shift 3
            ;;
        --output)
            if (($# < 2)); then
                printf '%s\n' "--output requires a path." >&2
                exit 2
            fi
            output_path="$2"
            shift 2
            ;;
        -h|--help)
            printf '%s\n' "Usage: $0 [--output PATH] [--hover X Y]"
            exit 0
            ;;
        *)
            if [[ "$output_path" != "artifacts/screenshots/cadence-native.png" ]]; then
                printf '%s\n' "Only one screenshot output path may be supplied." >&2
                exit 2
            fi
            output_path="$1"
            shift
            ;;
    esac
done

if [[ "$output_path" != /* ]]; then
    output_path="$project_dir/$output_path"
fi
mkdir -p "$(dirname -- "$output_path")"

window_id() {
    swift -e '
import CoreGraphics

let windows = CGWindowListCopyWindowInfo(
    [.optionAll, .excludeDesktopElements],
    kCGNullWindowID
) as? [[String: Any]] ?? []

for window in windows {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    let name = window[kCGWindowName as String] as? String ?? ""
    let id = window[kCGWindowNumber as String] as? Int ?? 0
    if owner == "cadence-native" && name == "Cadence — local track review" {
        print(id)
        break
    }
}
' 2>/dev/null | head -n 1
}

log_path="$(mktemp -t cadence-native-screenshot.XXXXXX)"
app_pid=""

cleanup() {
    if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    fi
    rm -f "$log_path"
}
trap cleanup EXIT INT TERM

native_window_id="$(window_id)"
if [[ -z "$native_window_id" ]]; then
    (
        cd "$project_dir"
        cargo build --quiet --locked
        exec target/debug/cadence-native
    ) >"$log_path" 2>&1 &
    app_pid=$!

    for _ in {1..60}; do
        sleep 0.5
        native_window_id="$(window_id)"
        [[ -n "$native_window_id" ]] && break
    done
fi

if [[ -z "$native_window_id" ]]; then
    printf '%s\n' "Cadence native window did not appear." >&2
    tail -n 40 "$log_path" >&2
    exit 1
fi

osascript -e 'tell application "System Events" to set frontmost of process "cadence-native" to true' 2>/dev/null || true

if [[ -n "$hover_x" ]]; then
    swift -e '
import CoreGraphics
import Foundation

let x = Double(CommandLine.arguments[1]) ?? 0.0
let y = Double(CommandLine.arguments[2]) ?? 0.0
let point = CGPoint(x: x, y: y)
let event = CGEvent(
    mouseEventSource: nil,
    mouseType: .mouseMoved,
    mouseCursorPosition: point,
    mouseButton: .left
)
event?.post(tap: .cghidEventTap)
' "$hover_x" "$hover_y"
    sleep 0.2
fi

screencapture -x -o -l "$native_window_id" "$output_path"
printf 'Captured Cadence native window %s to %s\n' "$native_window_id" "$output_path"
