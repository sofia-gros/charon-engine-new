"""Validate stored profiling pairs without making timing assertions in CI."""
import json
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
folder = root / "docs/performance"
before = json.loads((folder / "m2-copy-baseline.json").read_text(encoding="utf-8"))
after = json.loads((folder / "m2-buffer-handoff.json").read_text(encoding="utf-8"))

def keyed(report):
    result = {}
    for case in report["samples"]:
        key = (case["width"], case["height"], case["scene"], case["layer"])
        if key in result or not re.fullmatch(r"[0-9a-f]{16}", case["state_checksum"]):
            raise SystemExit(f"Invalid or duplicated case: {key}")
        result[key] = case
    expected = {(w,h,scene,layer) for w,h in [(365,365),(640,360),(1280,720)]
                for scene in ["falling_sand","settled_stone","liquid","reactions"]
                for layer in ["kernel","simulation_snapshot","plugin_snapshot"]}
    if result.keys() != expected:
        raise SystemExit("Incomplete profile case matrix")
    if not re.fullmatch(r"[0-9a-f]{64}", report["environment"]["physics_source_sha256"]):
        raise SystemExit("Missing physics source fingerprint")
    return result

left, right = keyed(before), keyed(after)
for key, a in left.items():
    b = right[key]
    for field in ["seed","trials","ticks_per_trial","state_checksum","mean_active_chunks","max_active_chunks","grid_buffer_bytes","snapshot_cell_bytes"]:
        if a[field] != b[field]:
            raise SystemExit(f"Profile comparison differs: {key} {field}")
print("36 before/after profile cases: seeds, state checksums, chunk schedules, and buffer sizes match")
