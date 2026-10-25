"""Keep the M1 core free of optional game/platform dependencies."""
import json
import subprocess

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
    text=True,
))
core = next(package for package in metadata["packages"] if package["name"] == "charon-core")
if core["dependencies"]:
    raise SystemExit("charon-core must depend only on std: " + repr(core["dependencies"]))
print("charon-core: std only; no game, renderer, GPU, or window dependencies")
