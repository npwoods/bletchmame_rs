#!/usr/bin/env python3
"""Set the crate version (and publication settings) from the version reported by git."""

import re
import subprocess
from pathlib import Path


VERSION_PATTERN = re.compile(
    r"^v([0-9]+)\.([0-9]+)(?:-([0-9]+)-g[0-9a-f]+)?(?:-dirty)?$"
)
PUBLISH_LINE = 'publish = ["crates-io"]\n'


def add_publish_setting(root: Path) -> None:
    """Add `publish`, which Cargo only permits once `version` has been set."""
    manifest_path = root / "Cargo.toml"
    manifest = manifest_path.read_text(encoding="utf-8")
    if PUBLISH_LINE in manifest:
        return
    package_header = "[package]\n"
    if package_header not in manifest:
        raise RuntimeError("Cannot find [package] section in Cargo.toml")
    manifest = manifest.replace(package_header, package_header + PUBLISH_LINE, 1)
    manifest_path.write_text(manifest, encoding="utf-8")


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
    add_publish_setting(root)


if __name__ == "__main__":
    main()
