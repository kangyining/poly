"""Verify source snapshots and Git histories without running application code."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def git(*args: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(ROOT), *args], text=True, encoding="utf-8"
    ).strip()


def main() -> None:
    manifest = json.loads((ROOT / "docs/source-repositories.json").read_text())
    retained = set(git("rev-list", "--all").splitlines())
    checked_branches = 0
    source_commits: set[str] = set()
    results = []
    for source in manifest["sources"]:
        name = source["name"]
        imported_tree = git("rev-parse", f"main:{name}")
        if imported_tree != source["default_tree"]:
            raise SystemExit(f"Imported tree differs from source: {name}")
        for branch, sha in source["branches"].items():
            ref = f"refs/heads/archive/{name}/{branch}"
            if git("rev-parse", ref) != sha:
                raise SystemExit(f"Archived branch differs from source: {ref}")
            checked_branches += 1
        for tag, sha in source["tags"].items():
            ref = f"refs/tags/source/{name}/{tag}"
            if git("rev-parse", ref) != sha:
                raise SystemExit(f"Archived tag differs from source: {ref}")
        missing = set(source["all_commits"]) - retained
        if missing:
            raise SystemExit(f"Missing source commits for {name}: {sorted(missing)}")
        source_commits.update(source["all_commits"])
        results.append({"project": name, "tree_matches": True})
    print(json.dumps({
        "projects": results,
        "archived_branches": checked_branches,
        "original_commits_retained": len(source_commits),
        "status": "verified",
    }, indent=2))


if __name__ == "__main__":
    main()
