#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/env.sh"
cd "$repo_root"

require_project() {
  if [ ! -f Cargo.toml ] || [ ! -f Cargo.lock ]; then
    printf '%s\n' 'Engine checks unavailable: Cargo.toml and a committed Cargo.lock are required.' >&2
    exit 2
  fi
}

case "${1:-help}" in
  doctor)
    rustc --version
    cargo --version
    cargo fmt --version
    cargo clippy --version
    rust-analyzer --version
    cc --version | head -n 1
    python3 --version
    doctor_dir="$(mktemp -d)"
    trap 'rm -rf "$doctor_dir"' EXIT
    printf '%s\n' 'fn main() { println!("charon-toolchain-ok"); }' > "$doctor_dir/main.rs"
    rustc --edition 2021 "$doctor_dir/main.rs" -o "$doctor_dir/smoke"
    test "$("$doctor_dir/smoke")" = charon-toolchain-ok
    printf '%s\n' 'Toolchain compile/link/run passed; engine and GPU performance are not tested.'
    ;;
  check)
    if [ "$#" -lt 2 ] || [ "$#" -gt 3 ] || [[ "$2" == -* ]]; then
      printf '%s\n' 'Usage: bash scripts/dev.sh check PACKAGE [TEST_FILTER]' >&2
      exit 2
    fi
    require_project
    cargo check --locked -p "$2" --all-targets
    cargo test --locked -p "$2" "${3:-}"
    cargo clippy --locked -p "$2" --all-targets -- -D warnings
    cargo fmt --all -- --check
    ;;
  full)
    require_project
    cargo fmt --all -- --check
    cargo check --locked --workspace --all-targets
    cargo test --locked --workspace
    cargo clippy --locked --workspace --all-targets -- -D warnings
    ;;
  smoke)
    require_project
    if [ -d /workspace/.cloud-tools ] && [ -z "${MESA_SHADER_CACHE_DIR:-}" ]; then
      export MESA_SHADER_CACHE_DIR=/workspace/.cloud-tools/mesa-cache
      mkdir -p "$MESA_SHADER_CACHE_DIR"
    fi
    if [ -z "${VK_DRIVER_FILES:-}" ] && [ ! -d /dev/dri ] && [ -f /usr/lib/chromium/vk_swiftshader_icd.json ]; then
      export VK_DRIVER_FILES=/usr/lib/chromium/vk_swiftshader_icd.json
    fi
    if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
      smoke_runtime_dir="$(mktemp -d)"
      export XDG_RUNTIME_DIR="$smoke_runtime_dir"
      trap 'rm -rf "$smoke_runtime_dir"' EXIT
    fi
    cargo run --locked -p charon-sandbox -- --smoke --output artifacts/local/smoke.png
    ;;
  bench)
    if [ "$#" -ne 3 ] || [[ "$2" == -* ]] || [[ "$3" == -* ]]; then
      printf '%s\n' 'Usage: bash scripts/dev.sh bench PACKAGE BENCH_TARGET' >&2
      exit 2
    fi
    require_project
    cargo bench --locked -p "$2" --bench "$3"
    ;;
  help)
    printf '%s\n' 'doctor | check PACKAGE [TEST_FILTER] | full | smoke | bench PACKAGE BENCH_TARGET'
    ;;
  *)
    printf '%s\n' 'Unknown command; run: bash scripts/dev.sh help' >&2
    exit 2
    ;;
esac
