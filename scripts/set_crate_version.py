#!/usr/bin/env python3
"""Set the crate version from the version reported by git."""

import re
import subprocess
from pathlib import Path


VERSION_PATTERN = re.compile(
    r"^v([0-9]+)\.([0-9]+)(?:-([0-9]+)-g[0-9a-f]+)?(?:-dirty)?$"
)


def main():
    root = Path(__file__).resolve().parent.parent
    git_description = subprocess.run(
        ["git", "describe", "--tags"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    match = VERSION_PATTERN.fullmatch(git_description)
    if match is None:
        raise RuntimeError(f"Cannot process build string: {git_description}")

    major, minor, build = match.groups()
    version = ".".join(part for part in (major, minor, build) if part is not None)
    crate_version = version if build is not None else f"{version}.0"
    subprocess.run(["cargo", "set-version", crate_version], cwd=root, check=True)


if __name__ == "__main__":
    main()
