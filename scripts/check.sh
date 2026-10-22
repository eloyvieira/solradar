#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if command -v cargo >/dev/null; then cargo fmt --all -- --check; cargo test --workspace; cargo check --workspace;
else echo 'Cargo não instalado. Use um ambiente Rust para validar build/testes.'; exit 2; fi
