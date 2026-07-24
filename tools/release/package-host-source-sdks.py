#!/usr/bin/env python3

import argparse
import gzip
import json
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[2]


def git(*arguments):
    return subprocess.check_output(
        ["git", *arguments],
        cwd=ROOT,
        text=True,
    ).strip()


def tracked_files(path):
    listing = subprocess.check_output(
        ["git", "ls-files", "-z", "--", path],
        cwd=ROOT,
    )
    for raw in listing.split(b"\0"):
        if raw:
            yield Path(raw.decode())


def copy_tracked(source, destination, flatten):
    files = list(tracked_files(source))
    if not files:
        raise ValueError(f"no tracked files under {source}")
    base = Path(source)
    for relative in files:
        target_relative = relative.relative_to(base) if flatten else relative
        target = destination / target_relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, target)


def archive_tree(source, output, prefix):
    with output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(
                mode="w",
                fileobj=compressed,
                format=tarfile.PAX_FORMAT,
            ) as archive:
                for path in sorted(source.rglob("*")):
                    if path.is_symlink():
                        raise ValueError(f"source package contains a symlink: {path}")
                    if not path.is_file():
                        continue
                    relative = path.relative_to(source).as_posix()
                    info = archive.gettarinfo(
                        str(path),
                        arcname=str(PurePosixPath(prefix, relative)),
                    )
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    info.mtime = 0
                    with path.open("rb") as contents:
                        archive.addfile(info, contents)


def add_common(destination):
    shutil.copy2(
        ROOT / "prns-host" / "distribution" / "PACKAGE.md",
        destination / "PACKAGE.md",
    )
    shutil.copy2(ROOT / "LICENSE-APACHE", destination / "LICENSE-APACHE")
    shutil.copy2(ROOT / "LICENSE-MIT", destination / "LICENSE-MIT")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--julia-artifacts", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--expected-sha", required=True)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    commit = git("rev-parse", "HEAD")
    if commit != args.expected_sha:
        raise SystemExit("checked-out commit differs from --expected-sha")
    if not args.allow_dirty and git("status", "--porcelain"):
        raise SystemExit("source SDK staging requires a clean worktree")
    julia_artifacts = Path(args.julia_artifacts).resolve()
    if not julia_artifacts.is_file():
        raise SystemExit("generated Julia Artifacts.toml does not exist")
    output = Path(args.output).resolve()
    if output.exists() and any(output.iterdir()):
        raise SystemExit(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    version = (ROOT / "VERSION").read_text().strip()
    with tempfile.TemporaryDirectory(prefix="prns-source-sdks-") as temporary:
        staging = Path(temporary)
        go = staging / "go"
        go.mkdir()
        copy_tracked("prns-host/bindings/go", go, flatten=True)
        add_common(go)
        archive_tree(
            go,
            output / f"personal-rns-{version}-go.tar.gz",
            f"personal-rns-go-{version}",
        )
        swift = staging / "swift"
        swift.mkdir()
        shutil.copy2(ROOT / "Package.swift", swift / "Package.swift")
        copy_tracked("prns-host/bindings/swift", swift, flatten=False)
        add_common(swift)
        archive_tree(
            swift,
            output / f"personal-rns-{version}-swift.tar.gz",
            f"personal-rns-swift-{version}",
        )
        julia = staging / "julia"
        julia.mkdir()
        copy_tracked("prns-host/bindings/julia", julia, flatten=True)
        shutil.copy2(julia_artifacts, julia / "Artifacts.toml")
        add_common(julia)
        archive_tree(
            julia,
            output / f"personal-rns-{version}-julia.tar.gz",
            f"personal-rns-julia-{version}",
        )
    manifest = {
        "format": 1,
        "version": version,
        "commit": commit,
        "tags": {
            "go": f"prns-host/bindings/go/v{version}",
            "swift": f"v{version}",
            "julia": f"PersonalRns-v{version}",
        },
    }
    (output / "source-sdks.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    )


if __name__ == "__main__":
    main()
