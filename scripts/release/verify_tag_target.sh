#!/usr/bin/env bash
set -euo pipefail

repository="${GITHUB_REPOSITORY:-}"
tag=""
source_sha="${SOURCE_SHA:-}"
max_peel_depth="${TAG_TARGET_MAX_PEEL_DEPTH:-8}"

usage() {
    cat <<'EOF'
Usage: verify_tag_target.sh --repository OWNER/REPOSITORY --tag TAG --source-sha SHA [--max-peel-depth N]

Resolves a GitHub lightweight or annotated tag and requires its final commit to
match SHA. Annotated tag peeling is bounded to prevent cycles or unbounded API
walks.
EOF
}

fail() {
    echo "release tag target verification failed: $1" >&2
    exit 1
}

while (($# > 0)); do
    case "$1" in
        --repository|-r)
            [[ $# -ge 2 ]] || { echo "--repository requires a value" >&2; exit 2; }
            repository="$2"
            shift 2
            ;;
        --tag)
            [[ $# -ge 2 ]] || { echo "--tag requires a value" >&2; exit 2; }
            tag="$2"
            shift 2
            ;;
        --source-sha)
            [[ $# -ge 2 ]] || { echo "--source-sha requires a value" >&2; exit 2; }
            source_sha="$2"
            shift 2
            ;;
        --max-peel-depth)
            [[ $# -ge 2 ]] || { echo "--max-peel-depth requires a value" >&2; exit 2; }
            max_peel_depth="$2"
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

[[ "$repository" =~ ^[^/[:space:]]+/[^/[:space:]]+$ ]] || fail "repository must be OWNER/REPOSITORY"
[[ -n "$tag" && "$tag" != *$'\r'* && "$tag" != *$'\n'* && "$tag" != *$'\t'* ]] || fail "tag is missing or contains control characters"
[[ "$source_sha" =~ ^[0-9a-fA-F]{40}$ ]] || fail "SOURCE_SHA must be a full commit SHA"
source_sha="$(printf '%s' "$source_sha" | tr '[:upper:]' '[:lower:]')"
[[ "$max_peel_depth" =~ ^[1-9][0-9]*$ && "$max_peel_depth" -le 32 ]] || fail "max peel depth must be an integer from 1 through 32"

read_target() {
    local endpoint="$1"
    local line
    local extra

    if ! line="$(gh api \
        --header 'Accept: application/vnd.github+json' \
        --jq '[.object.type, .object.sha] | @tsv' \
        "$endpoint" 2>/dev/null
    )"; then
        fail "tag target API lookup failed"
    fi
    [[ "$line" != *$'\r'* && "$line" != *$'\n'* && "$line" == *$'\t'* ]] || fail "tag target API response is malformed"
    IFS=$'\t' read -r target_type target_sha extra <<<"$line"
    [[ -z "${extra:-}" && -n "${target_type:-}" && -n "${target_sha:-}" ]] || fail "tag target API response is malformed"
    case "$target_type" in
        commit|tag)
            ;;
        *)
            fail "tag target type is not a commit or annotated tag"
            ;;
    esac
    [[ "$target_sha" =~ ^[0-9a-fA-F]{40}$ ]] || fail "tag target SHA is malformed"
    target_sha="$(printf '%s' "$target_sha" | tr '[:upper:]' '[:lower:]')"
}

seen_shas=""
remember_sha() {
    seen_shas+="$1"$'\n'
}

has_seen_sha() {
    local candidate="$1"
    local seen
    while IFS= read -r seen; do
        [[ -n "$seen" && "$seen" == "$candidate" ]] && return 0
    done <<<"$seen_shas"
    return 1
}

read_target "repos/$repository/git/ref/tags/$tag"
remember_sha "$target_sha"
peel_depth=0
while [[ "$target_type" == tag ]]; do
    if ((peel_depth >= max_peel_depth)); then
        fail "annotated tag peel depth exceeds the configured bound"
    fi
    read_target "repos/$repository/git/tags/$target_sha"
    if has_seen_sha "$target_sha"; then
        fail "annotated tag target cycle detected"
    fi
    remember_sha "$target_sha"
    peel_depth=$((peel_depth + 1))
done

[[ "$target_sha" == "$source_sha" ]] || fail "tag does not resolve to SOURCE_SHA"
printf '%s\n' "verified $tag -> $source_sha"
