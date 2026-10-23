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

# Explicit software-compute adapter for GPU-less managed Cloud sessions.
# Existing driver selections and machines with a GPU are left to the system.
if [ -d /workspace/.cloud-tools ] && [ -z "${VK_DRIVER_FILES:-}" ] && [ ! -d /dev/dri ] && [ -f /usr/lib/chromium/vk_swiftshader_icd.json ]; then
  export VK_DRIVER_FILES=/usr/lib/chromium/vk_swiftshader_icd.json
fi
