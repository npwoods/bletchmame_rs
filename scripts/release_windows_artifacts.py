#!/usr/bin/env python3
"""Build the Windows MSI and ZIP release artifacts."""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def run(command, **kwargs):
    print("+", " ".join(str(part) for part in command), file=sys.stderr)
    subprocess.run(
        command,
        check=True,
        stdout=sys.stderr,
        stderr=sys.stderr,
        **kwargs,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", help="Release version, for example 3.0")
    args = parser.parse_args()

    root = Path(__file__).resolve().parent.parent
    version = args.version
    if version is None:
        version = read_cargo_version(root)
    if version.endswith(".0"):
        version = version[:-2]
    artifact_name = f"BletchMAME_{version.replace('.', '_')}"
    version_underscores = version.replace(".", "_")
    temporary_directory = root / "rel" / "temp"
    zip_directory = root / "rel" / "zip" / artifact_name
    msi_directory = root / "rel" / "msi"
    temporary_directory.mkdir(parents=True, exist_ok=True)
    zip_directory.mkdir(parents=True, exist_ok=True)
    msi_directory.mkdir(parents=True, exist_ok=True)

    magick = shutil.which("magick")
    if magick is None:
        raise RuntimeError("ImageMagick's magick executable was not found")

    pandoc = shutil.which("pandoc")
    if pandoc is None:
        raise RuntimeError("Pandoc's pandoc executable was not found")

    candle = shutil.which("candle")
    light = shutil.which("light")
    if candle is None or light is None:
        raise RuntimeError("WiX candle and light executables must be on PATH")

    icon = temporary_directory / "bletchmame.ico"
    dialog_bitmap = temporary_directory / "bletchmame.bmp"
    wixobj = temporary_directory / "bletchmame.wixobj"
    msi = temporary_directory / "BletchMAME.msi"

    run([
        pandoc,
        str(root / "README.md"),
        "-o",
        str(root / "readme.html"),
    ], cwd=root)
    run([
        magick,
        str(root / "ui" / "icons" / "bletchmame.png"),
        "-define",
        "icon:auto-resize=256,128,64,48,32,16",
        str(icon),
    ])
    run([
        magick,
        str(root / "ui" / "icons" / "bletchmame.png"),
        "-background",
        "white",
        "-alpha",
        "remove",
        str(dialog_bitmap),
    ])
    run([
        candle,
        "-v",
        str(root / "bletchmame.wxs"),
        f"-dVERSION={version}",
        f"-dICON={icon}",
        f"-dDIALOG_BMP={dialog_bitmap}",
        "-out",
        str(wixobj),
    ], cwd=root)
    run([
        light,
        "-v",
        "-ext",
        "WixUIExtension",
        "-ext",
        "WixUtilExtension",
        str(wixobj),
        "-out",
        str(msi),
    ], cwd=root)

    shutil.copy2(msi, msi_directory / f"BletchMAME_{version_underscores}.msi")

    shutil.copy2(
        root / "target" / "release" / "BletchMAME.exe",
        zip_directory / "BletchMAME.exe",
    )
    shutil.copy2(root / "readme.html", zip_directory / "readme.html")
    shutil.copytree(root / "plugins", zip_directory / "plugins", dirs_exist_ok=True)
    print(artifact_name)


def read_cargo_version(root):
    import tomllib

    with (root / "Cargo.toml").open("rb") as cargo_toml:
        return tomllib.load(cargo_toml)["package"]["version"]


if __name__ == "__main__":
    main()
