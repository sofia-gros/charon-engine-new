"""Check that tracked demo GIFs have metadata and README embeds (stdlib only)."""
import json
from pathlib import Path
import re
import struct

root = Path(__file__).resolve().parents[1]
readme = (root / "README.md").read_text(encoding="utf-8")
files = sorted((root / "docs/demos").glob("m*-demo.gif"))
if not files:
    raise SystemExit("No milestone GIFs found")
for path in files:
    data = path.read_bytes()
    if len(data) < 14 or data[:6] not in (b"GIF87a", b"GIF89a") or data[-1] != 0x3B:
        raise SystemExit(f"Invalid GIF: {path.name}")
    width, height = struct.unpack("<HH", data[6:10])
    if not width or not height:
        raise SystemExit(f"Empty GIF dimensions: {path.name}")
    metadata = json.loads(path.with_suffix(".json").read_text(encoding="utf-8"))
    required = {"milestone", "status", "sample_commit", "recorded_at", "environment", "display", "windows_11_verified", "sample_command", "actions"}
    if required - metadata.keys() or not metadata["actions"]:
        raise SystemExit(f"Incomplete metadata: {path.name}")
    if not re.fullmatch(r"[a-f0-9]{40}", metadata["sample_commit"]):
        raise SystemExit(f"Invalid source commit: {path.name}")
    if not re.search(r"!\[[^\]]*\]\(\./docs/demos/" + re.escape(path.name) + r"\)", readme):
        raise SystemExit(f"Missing README image embed: {path.name}")
    print(f"{path.name}: {width}x{height}, {len(data)} bytes, metadata and README embed OK")
