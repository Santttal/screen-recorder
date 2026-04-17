#!/usr/bin/env bash
set -euo pipefail

export RUST_LOG="${RUST_LOG:-debug,screen_record=trace}"
export GST_DEBUG="${GST_DEBUG:-2}"
export PATH="$HOME/.cargo/bin:$PATH"

cd "$(dirname "$0")/.."
exec cargo run "$@"
