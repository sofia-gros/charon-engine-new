# Source this file in each shell. Do not run it as a separate process.
# Local rustup installations continue to use their own homes.
if [ -x /workspace/.cloud-tools/cargo/bin/rustup ]; then
  export CARGO_HOME=/workspace/.cloud-tools/cargo
  export RUSTUP_HOME=/workspace/.cloud-tools/rustup
  case ":$PATH:" in
    *":$CARGO_HOME/bin:"*) ;;
    *) export PATH="$CARGO_HOME/bin:$PATH" ;;
  esac
fi
