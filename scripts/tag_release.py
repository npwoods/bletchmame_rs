#!/usr/bin/env python3
"""Tag the latest release in CHANGELOG.md if it is dated today."""

import re
import subprocess
import sys
from datetime import date
from pathlib import Path


RELEASE_PATTERN = re.compile(
    r"^## \[(?P<version>[0-9]+(?:\.[0-9]+)+)\] - (?P<date>[0-9]{4}-[0-9]{2}-[0-9]{2})$"
)


def latest_release(root: Path) -> tuple[str, date]:
    """Return the version and date of the first release section in CHANGELOG.md."""
    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    for line in changelog.splitlines():
        if line.startswith("## "):
            match = RELEASE_PATTERN.fullmatch(line)
            if match is None:
                raise RuntimeError(f"Cannot parse release heading: {line}")
            return match.group("version"), date.fromisoformat(match.group("date"))
    raise RuntimeError("Cannot find a release heading in CHANGELOG.md")


def main():
    root = Path(__file__).resolve().parent.parent
    version, release_date = latest_release(root)
    today = date.today()
    if release_date != today:
        raise RuntimeError(
            f"Latest release {version} is dated {release_date}, not today ({today})"
        )

    tag = f"v{version}"
    print("+", "git", "tag", tag, file=sys.stderr)
    subprocess.run(["git", "tag", tag], cwd=root, check=True)


if __name__ == "__main__":
    main()
