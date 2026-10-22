#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/env.sh"

if ! command -v rustup >/dev/null 2>&1; then
  if [ -d /workspace ]; then
    export CARGO_HOME=/workspace/.cloud-tools/cargo
    export RUSTUP_HOME=/workspace/.cloud-tools/rustup
    export PATH="$CARGO_HOME/bin:$PATH"
    mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
  fi
  installer="$(mktemp)"
  trap 'rm -f "$installer"' EXIT
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
    https://sh.rustup.rs -o "$installer"
  sh "$installer" -y --no-modify-path --profile minimal --default-toolchain stable
  source "$repo_root/scripts/env.sh"
  # rustup's default installation location on machines without /workspace.
  if ! command -v rustup >/dev/null 2>&1; then
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
  fi
fi
rustup toolchain install stable --profile minimal \
  --component rustfmt --component clippy --component rust-analyzer

cd "$repo_root"
bash scripts/dev.sh doctor
# Never generate a project or rewrite a lockfile during setup.
if [ -f Cargo.toml ] && [ -f Cargo.lock ]; then
  cargo fetch --locked
fi
python3 scripts/references.py
