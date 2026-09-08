#!/usr/bin/env python3
"""Install only verified members of the checked-in runtime artifact manifest."""

import argparse
import hashlib
import json
import pathlib
import shutil
import tarfile
import tempfile
import urllib.request
import zipfile


def download(artifact, destination):
    digest = hashlib.sha256()
    request = urllib.request.Request(artifact["url"], headers={"User-Agent": "rust-srec-image-build"})
    with urllib.request.urlopen(request, timeout=60) as response, destination.open("wb") as target:
        while chunk := response.read(1024 * 1024):
            digest.update(chunk)
            target.write(chunk)
    if digest.hexdigest() != artifact["sha256"]:
        raise ValueError(f"SHA256 mismatch: {artifact['url']}")


def member_name(names, suffix):
    matches = [name for name in names if name == suffix or name.endswith("/" + suffix)]
    if len(matches) != 1:
        raise ValueError(f"Expected one archive member ending in {suffix!r}, got {matches!r}")
    return matches[0]


def install(manifest, architecture, prefix):
    binaries = prefix / "bin"
    plugins = prefix / "share/streamlink/plugins"
    binaries.mkdir(parents=True, exist_ok=True)
    plugins.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="runtime-tools-") as temporary:
        for name, tool in manifest.items():
            artifact = tool.get(architecture, tool.get("common"))
            if artifact is None:
                raise ValueError(f"No {architecture} artifact for {name}")
            archive = pathlib.Path(temporary) / name
            download(artifact, archive)
            if tool["format"] == "file":
                shutil.copyfile(archive, plugins / "twitch.py")
                (plugins / "twitch.py").chmod(0o644)
                continue
            opener = tarfile.open if tool["format"] == "tar.xz" else zipfile.ZipFile
            with opener(archive) as package:
                names = package.getnames() if tool["format"] == "tar.xz" else package.namelist()
                for executable, suffix in tool["executables"].items():
                    member = member_name(names, suffix)
                    source = package.extractfile(member) if tool["format"] == "tar.xz" else package.open(member)
                    if source is None:
                        raise ValueError(f"Not a regular executable member: {member}")
                    with source, (binaries / executable).open("wb") as target:
                        shutil.copyfileobj(source, target)
                    (binaries / executable).chmod(0o755)
            print(f"Installed {name} {tool['version']} ({architecture}), SHA256 verified", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--architecture", choices=("amd64", "arm64"), required=True)
    parser.add_argument("--prefix", type=pathlib.Path, default=pathlib.Path("/usr/local"))
    arguments = parser.parse_args()
    manifest_path = pathlib.Path(__file__).with_name("runtime-tools.json")
    install(json.loads(manifest_path.read_text()), arguments.architecture, arguments.prefix)
