#!/usr/bin/env python3
"""Give the files that did not change the modification times of the last build.

    fork/ci-mtimes.py TREE RECORD

fork-release.yml keeps cargo's target directories between runs, and cargo
decides that a path crate (the app, the Tuta helper and the Tuta SDK) is stale
when a source file is newer than its last build. A fresh checkout stamps every
file with the checkout's time, so without this every one of those crates would
be recompiled on every run.

RECORD (JSON, kept in the same cache as the target directories) maps each file
under TREE to its SHA-256 and modification time at the last build. A file whose
content is unchanged gets that time back. A changed or new file keeps the
checkout's time, which is later than any build cargo recorded, so cargo
rebuilds exactly what changed. RECORD is then rewritten for the next run.
"""
import hashlib
import json
import os
import sys
from pathlib import Path

# What flatpak-builder does not copy into the build (the `skip` list
# tools/local-manifest.sh writes), and the state directory the record lives in.
SKIP = {
    ".git",
    ".flatpak-builder",
    "dist",
    "target",
    "packaging/out",
    "native/tuta-unofficial-sdk/target",
    "native/tuta-unofficial-sdk/upstream",
}


def files(tree):
    for root, dirs, names in os.walk(tree):
        rel_root = os.path.relpath(root, tree)
        dirs[:] = [d for d in dirs if os.path.normpath(os.path.join(rel_root, d)) not in SKIP]
        for name in names:
            path = os.path.join(root, name)
            if not os.path.islink(path):
                yield os.path.normpath(os.path.join(rel_root, name)), path


def digest(path):
    with open(path, "rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def main(tree, record_path):
    record_path = Path(record_path)
    try:
        previous = json.loads(record_path.read_text())
    except FileNotFoundError:
        previous = {}
    current = {}
    restored = 0
    for rel, path in files(tree):
        sha = digest(path)
        mtime = os.stat(path).st_mtime_ns
        old = previous.get(rel)
        if old is not None and old[0] == sha:
            mtime = old[1]
            os.utime(path, ns=(mtime, mtime))
            restored += 1
        current[rel] = [sha, mtime]
    record_path.parent.mkdir(parents=True, exist_ok=True)
    record_path.write_text(json.dumps(current))
    print(f"ci-mtimes: {restored} of {len(current)} files unchanged since the last build")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(*sys.argv[1:])
