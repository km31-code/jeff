#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR/desktop"
cargo test --manifest-path src-tauri/Cargo.toml history_storymap_full_session_check -- --nocapture
npm run test

echo "history_storymap_full_session_check completed"
