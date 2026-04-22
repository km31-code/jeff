#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR/desktop"
npm run lint
npm run build
npm run test
cargo build --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml

cd "$ROOT_DIR"
./scripts/verify_ipc_contract.sh

echo "Phase 2 checks passed"
