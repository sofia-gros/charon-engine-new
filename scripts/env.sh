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
# Optional cloud-local ALSA development files (the system runtime stays shared).
if [ -f /workspace/.cloud-tools/alsa/usr/lib/x86_64-linux-gnu/pkgconfig/alsa.pc ]; then
  export PKG_CONFIG_PATH="/workspace/.cloud-tools/alsa/usr/lib/x86_64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
fi
