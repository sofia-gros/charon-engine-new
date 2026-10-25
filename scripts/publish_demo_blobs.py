"""CI-only immutable media upload. Does not update branches, trees or commits."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.request

if os.environ.get("GITHUB_ACTIONS") != "true":
    raise SystemExit("Run only in the repository's trusted recording job")
if len(sys.argv)!=2 or sys.argv[1] not in ("m4","m5","m6"):
    raise SystemExit("Expected milestone m4, m5 or m6")
milestone=sys.argv[1]
repo = os.environ["GITHUB_REPOSITORY"]
manifest = {"source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), "files": []}
def upload(data):
    body = json.dumps({"content": base64.b64encode(data).decode(), "encoding": "base64"}).encode()
    request = urllib.request.Request(f"https://api.github.com/repos/{repo}/git/blobs", data=body, headers={"Authorization": "Bearer " + os.environ["GH_TOKEN"], "Accept": "application/vnd.github+json", "Content-Type": "application/json", "X-GitHub-Api-Version": "2022-11-28"}, method="POST")
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)["sha"]

for stem in [f"{milestone}-demo", f"{milestone}-low-demo" if milestone=="m6" else f"{milestone}-tilemap-demo"]:
    for extension in ["gif", "json"]:
        path = Path("docs/demos") / f"{stem}.{extension}"
        data = path.read_bytes()
        if len(data) > 5 * 1024 * 1024:
            raise SystemExit("Demo exceeds the 5 MiB recording budget")
        entry = {"path": str(path), "sha": upload(data), "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        manifest["files"].append(entry)
print("CHARON_DEMO_BLOBS " + json.dumps(manifest, separators=(",", ":")))
