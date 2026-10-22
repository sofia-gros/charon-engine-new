#!/usr/bin/env python3
"""Record fixed CPU workloads and their environment. Run after scripts/env.sh."""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import platform
import subprocess

ROOT = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("artifacts/local/benchmarks/sand"))
    args = parser.parse_args()
    result = subprocess.run(["cargo", "run", "--locked", "--release", "-p", "charon-sand", "--example", "measure"], cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True)
    rows = list(csv.DictReader(io.StringIO(result.stdout)))
    assert len(rows) == 24, "incomplete benchmark matrix"
    for width in ("480", "960"):
        for scene in ("sparse", "rain", "collapse", "settled"):
            group = [row for row in rows if row["width"] == width and row["scene"] == scene]
            assert {row["workers"] for row in group} == {"1", "2", "4"}
            assert len({(row["checksum"], row["final_sand"]) for row in group}) == 1
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock"]
    paths += sorted((ROOT / "engine/charon-runtime").rglob("*.rs"))
    paths += sorted((ROOT / "plugins/charon-sand").rglob("*.rs"))
    paths += [ROOT / "plugins/charon-sand/Cargo.toml", ROOT / "engine/charon-runtime/Cargo.toml"]
    hashes = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}
    def optional(path):
        path = Path(path)
        return path.read_text().strip() if path.is_file() else None
    cpu = optional("/proc/cpuinfo") or ""
    metadata = {
        "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "source_sha256": hashes,
        "rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True).strip(),
        "platform": platform.platform(),
        "cpu_model": next((line.split(":", 1)[1].strip() for line in cpu.splitlines() if line.startswith("model name")), platform.processor()),
        "cpu_quota": optional("/sys/fs/cgroup/cpu.max"),
        "memory_limit_bytes": optional("/sys/fs/cgroup/memory.max"),
        "profile": "release", "seed": "fixed integer inputs: x=8+(tick*37)%(width-16)",
        "scope": "CPU step only, no sleeping, window, GPU or upload timing",
        "samples_per_condition": 240, "warmup_ticks": 60,
        "checks": ["24 completed conditions", "matching final cells and sand counts across workers"],
    }
    output = ROOT / args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    output.with_suffix(".csv").write_text(result.stdout)
    output.with_suffix(".json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Recorded {output.with_suffix('.csv')} and .json")

if __name__ == "__main__":
    main()
