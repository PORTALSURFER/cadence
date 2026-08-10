#!/bin/zsh

set -u

APP_DIR="$(cd -- "$(dirname -- "$0")" && pwd -P)"
cd "$APP_DIR" || exit 1
exec cargo run -- "$@"
