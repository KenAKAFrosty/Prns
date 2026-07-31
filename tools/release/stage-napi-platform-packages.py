import argparse
import json
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PLATFORMS = {
    "darwin-arm64",
    "darwin-x64",
    "linux-arm64-gnu",
    "linux-arm64-musl",
    "linux-x64-gnu",
    "linux-x64-musl",
    "win32-arm64-msvc",
    "win32-x64-msvc",
}
DOCUMENTS = {
    "README.md": ROOT / "prns-host" / "distribution" / "PACKAGE.md",
    "LICENSE-APACHE": ROOT / "LICENSE-APACHE",
    "LICENSE-MIT": ROOT / "LICENSE-MIT",
}


def stage(packages):
    packages = Path(packages).resolve()
    actual = {path.name for path in packages.iterdir() if path.is_dir()}
    if actual != PLATFORMS:
        raise ValueError(
            f"expected platform packages {sorted(PLATFORMS)}, got {sorted(actual)}"
        )
    version = (ROOT / "VERSION").read_text().strip()
    for platform in sorted(PLATFORMS):
        directory = packages / platform
        manifest_path = directory / "package.json"
        manifest = json.loads(manifest_path.read_text())
        main = manifest.get("main")
        if (
            manifest.get("version") != version
            or not isinstance(main, str)
            or not main.endswith(".node")
            or Path(main).name != main
        ):
            raise ValueError(f"invalid generated package manifest: {manifest_path}")
        files = manifest.get("files")
        staged_files = [main, *DOCUMENTS]
        if not isinstance(files, list) or files not in ([main], staged_files):
            raise ValueError(f"unexpected generated package files: {manifest_path}")
        for name, source in DOCUMENTS.items():
            shutil.copy2(source, directory / name)
        manifest["files"] = staged_files
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--packages", required=True)
    args = parser.parse_args()
    try:
        stage(args.packages)
    except (FileNotFoundError, json.JSONDecodeError, ValueError) as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    main()
