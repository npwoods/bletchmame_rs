#!/usr/bin/env python3
import os
import sys
import shutil
import subprocess
import urllib.request
import argparse
from pathlib import Path

"""
Download the MAME self-extracting EXE for a specified release (e.g. mame0280),
extract it into ./mame, run mame.exe -createconfig, and list files.
Designed to be run from the repository root in CI on Windows.
"""


def main():
    parser = argparse.ArgumentParser(description='Download and setup MAME release')
    parser.add_argument('version', help='MAME release tag fragment, e.g. mame0280')
    parser.add_argument('--outdir', default='mame', help='Directory to extract into')
    args = parser.parse_args()

    repo_root = Path.cwd()
    version = args.version
    mame_url = f"https://github.com/mamedev/mame/releases/download/{version}/{version}b_64bit.exe"
    dest_sfx = repo_root / f"{version}_sfx.exe"
    mame_dir = repo_root / args.outdir

    print(f"Downloading {mame_url} -> {dest_sfx}")
    with urllib.request.urlopen(mame_url) as resp, open(dest_sfx, 'wb') as out:
        shutil.copyfileobj(resp, out)

    print(f"Creating directory {mame_dir}")
    mame_dir.mkdir(parents=True, exist_ok=True)

    # Try to extract with 7z if available
    try:
        subprocess.run(["7z", "x", str(dest_sfx), f"-o{mame_dir}", "-y"], check=True)
        extracted = True
    except Exception as e:
        print(f"7z extraction failed: {e}")
        extracted = False

    if not extracted:
        # Try running the self-extracting exe with -y -o<dir>
        try:
            subprocess.run([str(dest_sfx), "-y", f"-o{mame_dir}"], check=True)
            extracted = True
        except Exception as e:
            print(f"Self-extract run failed: {e}")
            extracted = False

    mame_exe = mame_dir / "mame.exe"
    if mame_exe.exists():
        print(f"Running {mame_exe} -createconfig")
        try:
            subprocess.run([str(mame_exe), "-createconfig"], check=True, cwd=str(mame_dir))
        except Exception as e:
            print(f"Failed to run mame.exe -createconfig: {e}")
    else:
        print("mame.exe not found after extraction")


if __name__ == '__main__':
    main()
