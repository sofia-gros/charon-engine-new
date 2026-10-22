#!/usr/bin/env python3
"""Restore pinned research sources outside the application checkout, without builds."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile


def snapshot_digest(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            kind, data = b"link", os.readlink(path).encode()
        elif path.is_file():
            kind, data = b"file", path.read_bytes()
        else:
            continue
        for value in (relative.encode(), kind, data):
            digest.update(len(value).to_bytes(8, "big"))
            digest.update(value)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path,
                        default=Path("/workspace/library-files/charon-references"))
    args = parser.parse_args()
    lock = Path(__file__).resolve().parent.parent / "docs/references.lock.json"
    entries = json.loads(lock.read_text())["repositories"]
    root = args.root.resolve()
    checkout = Path(__file__).resolve().parent.parent
    if root == checkout or checkout in root.parents:
        parser.error("reference sources must stay outside the application checkout")
    root.mkdir(parents=True, exist_ok=True)
    for entry in entries:
        destination = root / entry["name"]
        if destination.is_symlink():
            raise SystemExit(f"Refusing symlink destination: {destination}")
        if destination.exists():
            if snapshot_digest(destination) != entry["snapshot_sha256"]:
                raise SystemExit(f"Existing sources differ; preserved without changes: {destination}")
            print(f"Verified {entry['name']} @ {entry['commit']}")
            continue
        # Stage on the destination filesystem; publish only a verified complete tree.
        with tempfile.TemporaryDirectory(prefix=".charon-fetch-", dir=root) as temporary:
            staging = Path(temporary)
            bare = staging / "objects.git"
            sources = staging / "sources"
            sources.mkdir()
            subprocess.run(["git", "init", "--bare", "--quiet", str(bare)], check=True)
            subprocess.run(["git", "-C", str(bare), "fetch", "--quiet", "--depth=1",
                            f"https://github.com/{entry['repository']}.git", entry["commit"]],
                           check=True)
            actual = subprocess.check_output(
                ["git", "-C", str(bare), "rev-parse", "FETCH_HEAD^{commit}"], text=True).strip()
            if actual != entry["commit"]:
                raise SystemExit("Fetched revision does not match the reference lock")
            archive = staging / "source.tar"
            subprocess.run(["git", "-C", str(bare), "archive", "--format=tar",
                            f"--output={archive}", actual], check=True)
            with tarfile.open(archive) as bundle:
                bundle.extractall(sources, filter="data")
            if snapshot_digest(sources) != entry["snapshot_sha256"]:
                raise SystemExit(f"Reference checksum mismatch: {entry['name']}")
            sources.rename(destination)
            print(f"Restored {entry['name']} @ {actual}")


if __name__ == "__main__":
    main()
