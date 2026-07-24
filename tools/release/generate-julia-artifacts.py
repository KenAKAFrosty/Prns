#!/usr/bin/env python3

import argparse
import hashlib
import json
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "prns-host" / "distribution" / "packages.json"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as contents:
        for chunk in iter(lambda: contents.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_path(name):
    if "\\" in name:
        return False
    path = PurePosixPath(name)
    return not path.is_absolute() and ".." not in path.parts


def safe_zip_member(info):
    mode = info.external_attr >> 16
    file_type = mode & 0o170000
    return (
        safe_path(info.filename)
        and file_type in {0, 0o040000, 0o100000}
    )


def extract(archive, destination):
    if archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, mode="r:gz") as source:
            members = source.getmembers()
            if any(
                member.issym()
                or member.islnk()
                or not (member.isfile() or member.isdir())
                or not safe_path(member.name)
                for member in members
            ):
                raise ValueError(f"unsafe archive {archive}")
            source.extractall(destination, members=members)
        return
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as source:
            if any(not safe_zip_member(info) for info in source.infolist()):
                raise ValueError(f"unsafe archive {archive}")
            source.extractall(destination)
        return
    raise ValueError(f"unsupported artifact archive: {archive}")


def packaged_target(archive):
    if archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, mode="r:gz") as source:
            manifests = [
                member
                for member in source.getmembers()
                if member.isfile() and Path(member.name).name == "artifact.json"
            ]
            if len(manifests) != 1:
                raise ValueError(f"{archive} has no unique artifact manifest")
            contents = source.extractfile(manifests[0])
            if contents is None:
                raise ValueError(f"{archive} artifact manifest is unreadable")
            return json.load(contents)["target"]
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as source:
            manifests = [
                name
                for name in source.namelist()
                if Path(name).name == "artifact.json"
            ]
            if len(manifests) != 1:
                raise ValueError(f"{archive} has no unique artifact manifest")
            with source.open(manifests[0]) as contents:
                return json.load(contents)["target"]
    raise ValueError(f"unsupported artifact archive: {archive}")


def tree_hash(archive):
    with tempfile.TemporaryDirectory(prefix="prns-julia-artifact-") as temporary:
        directory = Path(temporary)
        extract(archive, directory)
        subprocess.run(["git", "init", "--quiet"], cwd=directory, check=True)
        subprocess.run(["git", "add", "--all"], cwd=directory, check=True)
        return subprocess.check_output(
            ["git", "write-tree"],
            cwd=directory,
            text=True,
        ).strip()


def toml_string(value):
    return json.dumps(value)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact", action="append", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--base-url")
    args = parser.parse_args()
    catalog = json.loads(CATALOG.read_text())
    version = (ROOT / "VERSION").read_text().strip()
    targets = {
        target["rust"]: target
        for target in catalog["nativeTargets"]
        if "julia" in target
    }
    provided = {}
    for value in args.artifact:
        target, separator, raw_path = value.partition("=")
        if not separator or target not in targets or target in provided:
            raise SystemExit(f"invalid or duplicate Julia artifact: {value}")
        path = Path(raw_path).resolve()
        if not path.is_file():
            raise SystemExit(f"Julia artifact does not exist: {path}")
        if packaged_target(path) != target:
            raise SystemExit(f"Julia artifact target differs from {target}: {path}")
        provided[target] = path
    if provided.keys() != targets.keys():
        missing = sorted(targets.keys() - provided.keys())
        raise SystemExit(f"missing Julia artifacts: {missing}")
    base_url = args.base_url or (
        "https://github.com/KenAKAFrosty/Prns/releases/download/"
        f"v{version}"
    )
    lines = []
    for target_name in sorted(provided):
        archive = provided[target_name]
        platform = targets[target_name]["julia"]
        lines.append("[[personal_rns]]")
        for key in ("arch", "os", "libc"):
            if key in platform:
                lines.append(f"{key} = {toml_string(platform[key])}")
        lines.append(f'git-tree-sha1 = "{tree_hash(archive)}"')
        lines.append("")
        lines.append("    [[personal_rns.download]]")
        lines.append(f'    sha256 = "{sha256(archive)}"')
        lines.append(
            f"    url = {toml_string(base_url + '/' + archive.name)}"
        )
        lines.append("")
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(lines))


if __name__ == "__main__":
    main()
