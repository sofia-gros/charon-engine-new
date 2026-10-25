"""Run repeatable release-mode physics windows and attach the measurement environment."""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--trials", type=int, default=12)
parser.add_argument("--ticks", type=int, default=16)
args = parser.parse_args()
if args.trials < 1 or args.ticks < 1:
    parser.error("trials and ticks must be positive")
with tempfile.TemporaryDirectory(prefix="charon-profile-") as temporary:
    raw = Path(temporary) / "results.json"
    subprocess.run(["cargo", "run", "--locked", "--release", "-p", "charon-bench", "--",
                    "--trials", str(args.trials), "--ticks", str(args.ticks), "--output", str(raw)], cwd=root, check=True)
    report = json.loads(raw.read_text(encoding="utf-8"))
report["environment"] = {
    "recorded_at": datetime.now(timezone(timedelta(hours=9))).isoformat(timespec="seconds"),
    "platform": platform.platform(), "machine": platform.machine(),
    "logical_cpus": os.cpu_count(),
    "cpu": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown") if Path("/proc/cpuinfo").exists() else platform.processor(),
    "rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True).strip(),
    "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
    "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True)),
    "physics_source_sha256": hashlib.sha256((root / "src/sim/physics.rs").read_bytes()).hexdigest(),
}
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"Saved {args.output}")
