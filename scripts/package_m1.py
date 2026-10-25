"""Package the native Windows sample and dependency license files after CI build."""
import json
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
destination = root / "dist" / "m1"
destination.mkdir(parents=True, exist_ok=True)
for source, name in [
    (root / "target/release/charon-m1-sample.exe", "charon-m1-sample.exe"),
    (root / "samples/m1-plugin-game/README.md", "README.md"),
    (root / "samples/m1-plugin-game/run.cmd", "run.cmd"),
    (root / "LICENSE", "LICENSE"),
]:
    shutil.copy2(source, destination / name)
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
(destination / "COMMIT.txt").write_text(commit + "\n", encoding="utf-8")

metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--format-version", "1",
    "--filter-platform", "x86_64-pc-windows-msvc",
], cwd=root, text=True))
packages = {p["id"]: p for p in metadata["packages"]}
nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
sample = next(p["id"] for p in packages.values() if p["name"] == "charon-m1-sample")
visited = set()
pending = [sample]
while pending:
    package_id = pending.pop()
    if package_id in visited:
        continue
    visited.add(package_id)
    pending.extend(dep["pkg"] for dep in nodes[package_id]["deps"])
    package = packages[package_id]
    if package["source"] is None:
        continue
    folder = Path(package["manifest_path"]).parent
    notices = [path for path in folder.iterdir() if path.is_file() and
               path.name.lower().startswith(("license", "licence", "copying", "notice", "copyright"))]
    if not notices:
        raise SystemExit(f"No dependency license found: {package['name']}")
    target = destination / "licenses" / f"{package['name']}-{package['version']}"
    target.mkdir(parents=True, exist_ok=True)
    for notice in notices:
        shutil.copy2(notice, target / notice.name)
print(f"Packaged {destination} for commit {commit}")
