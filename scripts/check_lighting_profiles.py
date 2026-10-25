"""Validate the saved M4 numerical/image manifest (no GPU or image library)."""
import hashlib
import json
from pathlib import Path
import struct

root = Path(__file__).resolve().parents[1]
data = json.loads((root / "docs/performance/m4-lighting.json").read_text())
assert len(data["cases"]) == 42
assert data["warmup_passes_per_resolution"] == 2
assert len(data["startup"]) == 6
assert len({(c["case"], c["cell_size"]) for c in data["cases"]}) == 42
assert {c["cell_size"] for c in data["cases"]} == {1, 2, 4}
for case in data["cases"]:
    assert 0 <= case["max_error"] <= data["tolerance"]
    assert case["upload_bytes"] == case["width"] * case["height"] * 48 + 64
    assert case["resident_bytes"] >= case["width"] * case["height"] * 116 + 64
    assert case["dispatches"] >= 2
    assert case["gpu_us"] is None or case["gpu_us"] >= 0
for path, expected in data["source_sha256"].items():
    assert hashlib.sha256((root / path).read_bytes()).hexdigest() == expected, path
assert len(data["image_sha256"]) == 12
for path, expected in data["image_sha256"].items():
    raw = (root / path).read_bytes()
    assert hashlib.sha256(raw).hexdigest() == expected, path
    assert raw[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", raw[16:24]) == (48, 32), path
print("M4: 42 numerical cases, source fingerprints and 12 CPU/GPU field PNGs OK")
