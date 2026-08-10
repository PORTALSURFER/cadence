#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
bundle_path="$project_dir/dist/Cadence.app"
executable_path=""
output_was_set=false
bundle_version=""
bundle_short_version=""
bundle_build_number=""
signing_identity="-"
production_signing=false

while (($# > 0)); do
    case "$1" in
        --executable)
            if (($# < 2)); then
                printf '%s\n' "--executable requires a path." >&2
                exit 2
            fi
            executable_path="$2"
            shift 2
            ;;
        --output)
            if (($# < 2)); then
                printf '%s\n' "--output requires a path." >&2
                exit 2
            fi
            bundle_path="$2"
            output_was_set=true
            shift 2
            ;;
        --version)
            if (($# < 2)); then
                printf '%s\n' "--version requires a value." >&2
                exit 2
            fi
            bundle_version="$2"
            shift 2
            ;;
        --bundle-short-version)
            if (($# < 2)); then
                printf '%s\n' "--bundle-short-version requires a value." >&2
                exit 2
            fi
            bundle_short_version="$2"
            shift 2
            ;;
        --bundle-build-number)
            if (($# < 2)); then
                printf '%s\n' "--bundle-build-number requires a value." >&2
                exit 2
            fi
            bundle_build_number="$2"
            shift 2
            ;;
        --signing-identity)
            if (($# < 2)); then
                printf '%s\n' "--signing-identity requires a value." >&2
                exit 2
            fi
            signing_identity="$2"
            production_signing=true
            shift 2
            ;;
        --)
            shift
            if (($# > 0)); then
                printf '%s\n' "Unexpected argument: $1" >&2
                exit 2
            fi
            ;;
        -*)
            printf '%s\n' "Unknown option: $1" >&2
            exit 2
            ;;
        *)
            if [[ "$output_was_set" == true ]]; then
                printf '%s\n' "Only one output path may be provided." >&2
                exit 2
            fi
            bundle_path="$1"
            output_was_set=true
            shift
            ;;
    esac
done

if [[ "$bundle_path" != /* ]]; then
    bundle_path="$project_dir/$bundle_path"
fi

case "$bundle_path" in
    *.app) ;;
    *)
        printf '%s\n' "Output path must end in .app: $bundle_path" >&2
        exit 2
        ;;
esac

if [[ "$(uname -s)" != "Darwin" ]]; then
    printf '%s\n' "Native macOS app bundles require macOS." >&2
    exit 1
fi

if [[ -n "$bundle_version" ]]; then
    release_version_re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(rc|nightly)\.[1-9][0-9]*)?$'
    if [[ ! "$bundle_version" =~ $release_version_re ]]; then
        printf '%s\n' "Version must use stable, rc, or nightly semantic version syntax: $bundle_version" >&2
        exit 2
    fi

    derived_short_version="${bundle_version%%-*}"
    if [[ -z "$bundle_short_version" ]]; then
        bundle_short_version="$derived_short_version"
    fi
    if [[ "$bundle_short_version" != "$derived_short_version" || ! "$bundle_short_version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
        printf '%s\n' "Bundle short version must be the numeric base version $derived_short_version." >&2
        exit 2
    fi
    if [[ -z "$bundle_build_number" ]]; then
        if [[ "$bundle_version" == *-* ]]; then
            bundle_build_number="${bundle_version##*.}"
        else
            bundle_build_number="$bundle_short_version"
        fi
    fi
    if [[ ! "$bundle_build_number" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
        printf '%s\n' "Bundle build number must contain only numeric components: $bundle_build_number" >&2
        exit 2
    fi
elif [[ -n "$bundle_short_version" || -n "$bundle_build_number" ]]; then
    printf '%s\n' "--bundle-short-version and --bundle-build-number require --version." >&2
    exit 2
fi

if [[ -z "$executable_path" ]]; then
    cargo build --release --locked --manifest-path "$project_dir/Cargo.toml"
    executable_path="$project_dir/target/release/cadence-native"
elif [[ "$executable_path" != /* ]]; then
    executable_path="$project_dir/$executable_path"
fi

if [[ ! -x "$executable_path" ]]; then
    printf '%s\n' "Executable is missing or not executable: $executable_path" >&2
    exit 1
fi

mkdir -p "$bundle_path/Contents/MacOS" "$bundle_path/Contents/Resources"
cp "$executable_path" "$bundle_path/Contents/MacOS/Cadence"
cp "$project_dir/assets/Cadence.icns" "$bundle_path/Contents/Resources/Cadence.icns"
cp "$project_dir/macos/Cadence/Info.plist" "$bundle_path/Contents/Info.plist"
printf 'APPL????' > "$bundle_path/Contents/PkgInfo"
chmod +x "$bundle_path/Contents/MacOS/Cadence"

if [[ -n "$bundle_version" ]]; then
    /usr/bin/plutil -replace CFBundleShortVersionString -string "$bundle_short_version" "$bundle_path/Contents/Info.plist"
    /usr/bin/plutil -replace CFBundleVersion -string "$bundle_build_number" "$bundle_path/Contents/Info.plist"
fi

/usr/bin/plutil -lint "$bundle_path/Contents/Info.plist" >/dev/null
codesign_args=(--force --deep --sign "$signing_identity")
if [[ "$production_signing" == true ]]; then
    codesign_args+=(--timestamp --options runtime)
fi
/usr/bin/codesign "${codesign_args[@]}" "$bundle_path" >/dev/null
/usr/bin/codesign --verify --deep --strict "$bundle_path"
/usr/bin/touch "$bundle_path"

printf 'Built %s\n' "$bundle_path"
