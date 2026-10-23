#!/usr/bin/env bash
# Managed Debian cloud fallback without root. Regular Linux: install libasound2-dev.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/env.sh"
if pkg-config --exists alsa; then exit 0; fi
if [ ! -d /workspace/.cloud-tools ] || [ "$(uname -m)" != x86_64 ]; then
  printf '%s\n' 'Install pkg-config and the ALSA development package (Debian/Ubuntu: libasound2-dev).' >&2
  exit 1
fi
alsa_root=/workspace/.cloud-tools/alsa
alsa_version="$(dpkg-query -W -f='${Version}' libasound2t64:amd64)"
mkdir -p "$alsa_root"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
  "https://deb.debian.org/debian/pool/main/a/alsa-lib/libasound2-dev_${alsa_version}_amd64.deb" -o "$alsa_root/dev.deb"
dpkg-deb -x "$alsa_root/dev.deb" "$alsa_root"
ln -sf /usr/lib/x86_64-linux-gnu/libasound.so.2 "$alsa_root/usr/lib/x86_64-linux-gnu/libasound.so"
sed -i "s|=/usr|=$alsa_root/usr|g" "$alsa_root/usr/lib/x86_64-linux-gnu/pkgconfig/alsa.pc"
