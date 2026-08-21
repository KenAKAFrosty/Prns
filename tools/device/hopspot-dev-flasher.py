from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
from types import ModuleType
from typing import Iterator, Sequence


DEVICE_TOOLS = Path(__file__).resolve().parent
if str(DEVICE_TOOLS) not in sys.path:
    sys.path.insert(0, str(DEVICE_TOOLS))

from developer_flasher_candidate import (
    DeveloperCandidateError,
    ExpectedTarget,
    ValidatedCandidate,
    validate_candidate,
)


ROOT = Path(__file__).resolve().parents[2]
WEBSITE = ROOT / "docs" / "website"
BOARD_CATALOG = ROOT / "release" / "flash" / "boards.json"
BOARD_CATALOG_SCHEMA = 4
SHIPPING_BOARD_AVAILABILITY = "shipping"
BOARD_AVAILABILITIES = frozenset((SHIPPING_BOARD_AVAILABILITY, "qualification"))
PINNED_MINISIGN = ROOT / ".build" / "toolchains" / "minisign" / "0.12" / "minisign"
MINISIGN_INSTALL_COMMAND = (
    "./tools/prns run release.toolchain.minisign.install -- "
    ".build/toolchains/minisign/0.12"
)
LOCAL_SECRET_KEY_MARKER = b"untrusted comment: minisign encrypted secret key"
LOCAL_SECRET_KEY_PLAIN_MARKER = b"untrusted comment: minisign secret key"
QUARANTINED_SOURCE_DIGESTS = frozenset(
    {
        "e3ffc728180a8194c2efb55f90b0285f093db6e53e6dc800d4b229426e966399",
    }
)


class DeveloperFlasherError(RuntimeError):
    pass


@dataclass(frozen=True)
class SourceIdentity:
    head: str
    digest: str
    state: str
    version: str


@dataclass(frozen=True)
class Selection:
    boards: tuple[str, ...]
    port: int


def process_output(value: bytes | None) -> str:
    return (value or b"").decode("utf-8", errors="replace").strip()


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    if os.name == "posix":
        os.killpg(process.pid, signal.SIGTERM)
    else:
        process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGKILL)
        else:
            process.kill()
        process.wait()


def run_process(
    command: Sequence[str | os.PathLike[str]],
    *,
    cwd: Path,
    environment: dict[str, str] | None = None,
    capture: bool = False,
    label: str,
) -> tuple[bytes, bytes]:
    process = subprocess.Popen(
        [os.fspath(part) for part in command],
        cwd=cwd,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        start_new_session=os.name == "posix",
    )
    try:
        stdout, stderr = process.communicate()
    except BaseException:
        stop_process(process)
        raise
    if process.returncode != 0:
        detail = "\n".join(
            value for value in (process_output(stdout), process_output(stderr)) if value
        )
        suffix = f": {detail}" if detail else ""
        raise DeveloperFlasherError(f"{label} failed with exit code {process.returncode}{suffix}")
    return stdout or b"", stderr or b""


def git_output(repository: Path, arguments: Sequence[str]) -> bytes:
    stdout, _ = run_process(
        ["git", *arguments],
        cwd=repository,
        capture=True,
        label=f"git {' '.join(arguments)}",
    )
    return stdout


def hash_length(digest: hashlib._Hash, value: int) -> None:
    digest.update(value.to_bytes(8, byteorder="big", signed=False))


def hash_bytes(digest: hashlib._Hash, value: bytes) -> None:
    hash_length(digest, len(value))
    digest.update(value)


def hash_worktree_entry(digest: hashlib._Hash, repository: Path, relative: bytes) -> None:
    hash_bytes(digest, relative)
    path = repository / os.fsdecode(relative)
    try:
        before = path.lstat()
    except FileNotFoundError:
        hash_bytes(digest, b"missing")
        return
    executable = b"executable" if before.st_mode & 0o111 else b"non-executable"
    hash_bytes(digest, executable)
    if stat.S_ISLNK(before.st_mode):
        hash_bytes(digest, b"symlink")
        hash_bytes(digest, os.fsencode(os.readlink(path)))
        return
    if stat.S_ISREG(before.st_mode):
        hash_bytes(digest, b"file")
        hash_length(digest, before.st_size)
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        after = path.stat()
        if (
            before.st_size,
            before.st_mtime_ns,
            before.st_mode,
        ) != (
            after.st_size,
            after.st_mtime_ns,
            after.st_mode,
        ):
            raise DeveloperFlasherError(f"source entry changed while hashing: {os.fsdecode(relative)}")
        return
    if stat.S_ISDIR(before.st_mode):
        hash_bytes(digest, b"gitlink")
        try:
            nested_head = git_output(path, ["rev-parse", "HEAD"]).strip()
            nested_status = git_output(
                path,
                ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            )
        except DeveloperFlasherError:
            nested_head = b"unavailable"
            nested_status = b""
        hash_bytes(digest, nested_head)
        hash_bytes(digest, nested_status)
        return
    raise DeveloperFlasherError(f"unsupported source entry type: {os.fsdecode(relative)}")


def source_identity(repository: Path) -> SourceIdentity:
    head = process_output(git_output(repository, ["rev-parse", "HEAD"]))
    if not re.fullmatch(r"[0-9a-f]{40}", head):
        raise DeveloperFlasherError("repository HEAD is not a lowercase full Git commit")
    entries = git_output(
        repository,
        ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    ).split(b"\0")
    entries = sorted(entry for entry in entries if entry)
    digest = hashlib.sha256()
    hash_bytes(digest, b"prns-local-dev-source-v1")
    hash_bytes(digest, head.encode("ascii"))
    for relative in entries:
        hash_worktree_entry(digest, repository, relative)
    status = git_output(
        repository,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    state = "dirty" if status else "clean"
    source_digest = digest.hexdigest()
    base_version = (repository / "VERSION").read_text(encoding="utf-8").strip()
    if not re.fullmatch(r"[A-Za-z0-9.+-]+", base_version) or base_version.lower() == "next":
        raise DeveloperFlasherError("repository VERSION is not an immutable path-safe identifier")
    return SourceIdentity(
        head=head,
        digest=source_digest,
        state=state,
        version=f"{base_version}-dev.{state}.{source_digest}",
    )


def require_unchanged_source(initial: SourceIdentity, final: SourceIdentity) -> None:
    if final != initial:
        raise DeveloperFlasherError(
            "working tree changed during the build; the unsigned candidate was discarded"
        )


def require_unquarantined_source(identity: SourceIdentity) -> None:
    if identity.digest in QUARANTINED_SOURCE_DIGESTS:
        raise DeveloperFlasherError(
            f"source digest {identity.digest} is quarantined after failed hardware qualification"
        )


def catalog_boards() -> tuple[dict, ...]:
    document = json.loads(BOARD_CATALOG.read_text(encoding="utf-8"))
    if document.get("schema") != BOARD_CATALOG_SCHEMA or not isinstance(
        document.get("boards"), list
    ):
        raise DeveloperFlasherError("board catalog is invalid")
    entries = document["boards"]
    if not entries or not all(isinstance(board, dict) for board in entries):
        raise DeveloperFlasherError("board catalog is invalid")
    if not all(board.get("availability") in BOARD_AVAILABILITIES for board in entries):
        raise DeveloperFlasherError("board catalog contains an invalid availability")
    slugs = tuple(board.get("slug") for board in entries)
    if not all(isinstance(board, str) and board for board in slugs):
        raise DeveloperFlasherError("board catalog contains an invalid slug")
    if len(set(slugs)) != len(slugs):
        raise DeveloperFlasherError("board catalog contains duplicate slugs")
    return tuple(entries)


def shipping_boards() -> tuple[str, ...]:
    return tuple(
        board["slug"]
        for board in catalog_boards()
        if board.get("availability") == SHIPPING_BOARD_AVAILABILITY
    )


def selected_targets(selection: Selection) -> tuple[ExpectedTarget, ...]:
    transports = {
        board["slug"]: board["transport"]
        for board in catalog_boards()
    }
    return tuple(ExpectedTarget(board, transports[board]) for board in selection.boards)


def parse_port(value: str) -> int:
    try:
        port = int(value, 10)
    except ValueError as error:
        raise argparse.ArgumentTypeError("--port must be an integer between 1 and 65535") from error
    if not 1 <= port <= 65535:
        raise argparse.ArgumentTypeError("--port must be between 1 and 65535")
    return port


def parse_selection(arguments: Sequence[str]) -> Selection:
    parser = argparse.ArgumentParser()
    parser.add_argument("boards", nargs="*")
    parser.add_argument("--all", action="store_true", dest="all_boards")
    parser.add_argument("--port", type=parse_port, default=8765)
    parsed = parser.parse_args(arguments)
    entries = catalog_boards()
    available = tuple(board["slug"] for board in entries)
    shipping = tuple(
        board["slug"]
        for board in entries
        if board.get("availability") == SHIPPING_BOARD_AVAILABILITY
    )
    if parsed.all_boards and parsed.boards:
        parser.error("--all cannot be combined with explicit boards")
    if parsed.all_boards:
        return Selection(shipping, parsed.port)
    if not parsed.boards:
        parser.error("select at least one cataloged board or use --all")
    if len(set(parsed.boards)) != len(parsed.boards):
        parser.error("board selections must be unique")
    unknown = sorted(set(parsed.boards) - set(available))
    if unknown:
        parser.error(f"unknown board: {', '.join(unknown)}")
    requested = set(parsed.boards)
    return Selection(tuple(board for board in available if board in requested), parsed.port)


def minisign_error(detail: str) -> DeveloperFlasherError:
    return DeveloperFlasherError(
        f"{detail}\nInstall the supported signer with exactly:\n{MINISIGN_INSTALL_COMMAND}"
    )


def executable_path(value: str) -> Path | None:
    if os.sep in value or (os.altsep is not None and os.altsep in value):
        candidate = Path(value)
        return candidate.resolve() if candidate.is_file() and os.access(candidate, os.X_OK) else None
    resolved = shutil.which(value)
    return Path(resolved).resolve() if resolved else None


def require_minisign(environment: dict[str, str]) -> Path:
    configured = environment.get("PRNS_MINISIGN_BIN")
    if configured:
        signer = executable_path(configured)
        if signer is None:
            raise minisign_error(f"PRNS_MINISIGN_BIN is unavailable: {configured}")
    elif PINNED_MINISIGN.is_file() and os.access(PINNED_MINISIGN, os.X_OK):
        signer = PINNED_MINISIGN.resolve()
    else:
        resolved = shutil.which("minisign", path=environment.get("PATH"))
        if resolved is None:
            raise minisign_error("Minisign 0.12 is required but was not found")
        signer = Path(resolved).resolve()
    stdout, stderr = run_process(
        [signer, "-v"],
        cwd=ROOT,
        environment=environment,
        capture=True,
        label="Minisign version check",
    )
    version = "\n".join(value for value in (process_output(stdout), process_output(stderr)) if value)
    if not re.search(r"(?:^|\s)minisign 0\.12(?:\s|$)", version):
        raise minisign_error(f"Minisign 0.12 is required; {signer} reported {version!r}")
    return signer


@contextmanager
def temporary_run_directory() -> Iterator[Path]:
    path = Path(tempfile.mkdtemp(prefix="prns-dev-flasher-"))
    path.chmod(0o700)
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


def clean_build_environment() -> dict[str, str]:
    environment = os.environ.copy()
    for name in tuple(environment):
        if (
            name.startswith("PRNS_LOCAL_DEV_")
            or name.startswith("PRNS_SOURCE_")
            or name
            in {
                "PRNS_BUILD_CHANNEL",
                "PRNS_BUILD_COMMIT",
                "PRNS_BUILD_SOURCE_DIGEST",
                "PRNS_BUILD_VERSION",
            }
        ):
            environment.pop(name)
    return environment


def clear_dioxus_output() -> Path:
    output = WEBSITE / "target" / "dx" / "reticulum-site" / "release" / "web" / "public"
    expected = WEBSITE / "target" / "dx" / "reticulum-site"
    if expected not in output.parents:
        raise DeveloperFlasherError(f"refusing unexpected Dioxus output path: {output}")
    shutil.rmtree(output, ignore_errors=True)
    return output


def build_firmware(
    candidate: Path,
    identity: SourceIdentity,
    selection: Selection,
    key_id: str,
) -> Path:
    environment = clean_build_environment()
    environment.update(
        {
            "PRNS_BUILD_COMMIT": identity.head,
            "PRNS_BUILD_SOURCE_DIGEST": identity.digest,
            "PRNS_BUILD_VERSION": identity.version,
        }
    )
    for board in selection.boards:
        run_process(
            [
                "cargo",
                "run",
                "--locked",
                "-p",
                "hopspot-flash",
                "--",
                "build",
                board,
                "--out-root",
                candidate,
                "--developer-version",
                identity.version,
            ],
            cwd=ROOT,
            environment=environment,
            label=f"{board} firmware build",
        )
    command: list[str | os.PathLike[str]] = [
        "cargo",
        "run",
        "--locked",
        "-p",
        "hopspot-flash",
        "--",
        "assemble-manifest",
        "--out-root",
        candidate,
        "--channel",
        "preview",
        "--commit",
        identity.head,
        "--key-id",
        key_id,
        "--developer-version",
        identity.version,
    ]
    for board in selection.boards:
        command.extend(["--board", board])
    run_process(
        command,
        cwd=ROOT,
        environment=environment,
        label="local development manifest assembly",
    )
    manifest = candidate / "flash-manifest.json"
    if not manifest.is_file():
        raise DeveloperFlasherError("manifest assembly did not produce flash-manifest.json")
    return manifest


def require_node_tools() -> tuple[Path, Path]:
    tailwind = WEBSITE / "node_modules" / ".bin" / "tailwindcss"
    esbuild = WEBSITE / "node_modules" / ".bin" / "esbuild"
    if tailwind.is_file() and esbuild.is_file():
        return tailwind, esbuild
    run_process(
        ["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"],
        cwd=WEBSITE,
        label="website dependency installation",
    )
    if not tailwind.is_file() or not esbuild.is_file():
        raise DeveloperFlasherError("website dependency installation did not provide build tools")
    return tailwind, esbuild


def sanitize_website_stage(website_stage: Path) -> None:
    for relative in (
        "source.zip",
        "source.zip.sha256",
        "flash-manifest.json",
        "firmware",
        "releases",
        "assets/flasher",
    ):
        path = website_stage / relative
        if path.is_dir():
            shutil.rmtree(path)
        else:
            path.unlink(missing_ok=True)


def build_website(
    website_stage: Path,
    identity: SourceIdentity,
    selection: Selection,
    public_key: Path,
) -> None:
    tailwind, esbuild = require_node_tools()
    output = clear_dioxus_output()
    environment = clean_build_environment()
    environment.update(
        {
            "PRNS_BUILD_VERSION": identity.version,
            "PRNS_BUILD_COMMIT": identity.head,
            "PRNS_BUILD_CHANNEL": "preview",
            "PRNS_LOCAL_DEV_PUBLIC_KEY": str(public_key),
            "PRNS_LOCAL_DEV_BOARDS": ",".join(selection.boards),
            "PRNS_LOCAL_DEV_SOURCE_DIGEST": identity.digest,
            "PRNS_LOCAL_DEV_SOURCE_STATE": identity.state,
        }
    )
    run_process(
        [
            "dx",
            "build",
            "--platform",
            "web",
            "--debug-symbols",
            "false",
            "--release",
            "--locked",
            "--features",
            "local-dev-flasher",
        ],
        cwd=WEBSITE,
        environment=environment,
        label="local developer website build",
    )
    if not (output / "index.html").is_file():
        raise DeveloperFlasherError("local developer website build did not produce index.html")
    shutil.copytree(output, website_stage, dirs_exist_ok=True)
    sanitize_website_stage(website_stage)
    assets = website_stage / "assets"
    flasher_assets = assets / "flasher"
    flasher_assets.mkdir(parents=True, exist_ok=True)
    run_process(
        [
            tailwind,
            "-i",
            WEBSITE / "tailwind.css",
            "-o",
            assets / "tailwind.css",
            "--minify",
        ],
        cwd=WEBSITE,
        label="local developer CSS build",
    )
    run_process(
        [
            esbuild,
            WEBSITE / "web-flasher" / "src" / "prns-flash.js",
            "--bundle",
            "--format=esm",
            "--platform=browser",
            "--target=es2022",
            "--minify",
            f"--outfile={flasher_assets / 'prns-flash.js'}",
        ],
        cwd=WEBSITE,
        label="local developer browser flasher build",
    )
    run_process(
        [
            "bash",
            ROOT / "tools" / "build" / "stage-web-flasher-nrf-dfu-wasm.sh",
            flasher_assets / "nrf-dfu",
        ],
        cwd=ROOT,
        label="local developer Nordic DFU browser core build",
    )


def generate_key(signer: Path, secrets: Path, environment: dict[str, str]) -> tuple[Path, Path, str]:
    public_key = secrets / "minisign.pub"
    secret_key = secrets / "minisign.key"
    run_process(
        [signer, "-G", "-p", public_key, "-s", secret_key, "-W"],
        cwd=secrets,
        environment=environment,
        capture=True,
        label="ephemeral Minisign key generation",
    )
    if not public_key.is_file() or not secret_key.is_file():
        raise DeveloperFlasherError("Minisign did not create the ephemeral key pair")
    secret_key.chmod(0o600)
    lines = public_key.read_text(encoding="utf-8").splitlines()
    if len(lines) != 2:
        raise DeveloperFlasherError("ephemeral Minisign public key is not canonical")
    match = re.fullmatch(r"untrusted comment: minisign public key ([0-9A-Fa-f]{16})", lines[0])
    if match is None or not re.fullmatch(r"[A-Za-z0-9+/]{56}", lines[1]):
        raise DeveloperFlasherError("ephemeral Minisign public key is not canonical")
    return public_key, secret_key, match.group(1).upper()


def sign_and_verify(
    signer: Path,
    document: Path,
    secret_key: Path,
    public_key: Path,
    environment: dict[str, str],
) -> Path:
    signature = document.with_name(f"{document.name}.minisig")
    trusted_comment = f"prns-local-dev-sha256:{hashlib.sha256(document.read_bytes()).hexdigest()}"
    run_process(
        [
            signer,
            "-S",
            "-s",
            secret_key,
            "-m",
            document,
            "-x",
            signature,
            "-t",
            trusted_comment,
        ],
        cwd=document.parent,
        environment=environment,
        capture=True,
        label=f"signing {document.name}",
    )
    if not signature.is_file():
        raise DeveloperFlasherError(f"Minisign did not create {signature.name}")
    run_process(
        [signer, "-Vm", document, "-x", signature, "-p", public_key],
        cwd=document.parent,
        environment=environment,
        capture=True,
        label=f"verification of {document.name}",
    )
    return signature


def write_channel_descriptor(candidate: Path, identity: SourceIdentity, manifest: Path) -> Path:
    channel = candidate / "channels" / "preview.json"
    channel.parent.mkdir(parents=True, exist_ok=True)
    document = {
        "schema": 1,
        "channel": "preview",
        "version": identity.version,
        "manifest_url": (
            f"https://reticulum.rs/releases/{identity.version}/flash-manifest.json"
        ),
        "manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
    }
    channel.write_text(
        json.dumps(document, indent=2, separators=(",", ": ")) + "\n",
        encoding="utf-8",
    )
    return channel


def verify_manifest_artifacts(
    candidate: Path,
    manifest_path: Path,
    identity: SourceIdentity,
    selection: Selection,
    key_id: str,
) -> ValidatedCandidate:
    try:
        return validate_candidate(
            candidate,
            manifest_path,
            identity.version,
            identity.head,
            identity.digest,
            key_id,
            selected_targets(selection),
        )
    except DeveloperCandidateError as error:
        raise DeveloperFlasherError(str(error)) from error


def stage_signed_release(
    website_stage: Path,
    validated: ValidatedCandidate,
    manifest: Path,
    manifest_signature: Path,
    channel: Path,
    channel_signature: Path,
    public_key: Path,
) -> None:
    releases = website_stage / "releases"
    release = releases / validated.version
    channels = releases / "channels"
    release.mkdir(parents=True, exist_ok=True)
    channels.mkdir(parents=True, exist_ok=True)
    shutil.copy2(public_key, releases / "minisign.pub")
    shutil.copy2(manifest, release / manifest.name)
    shutil.copy2(manifest_signature, release / manifest_signature.name)
    shutil.copy2(channel, channels / channel.name)
    shutil.copy2(channel_signature, channels / channel_signature.name)
    for target in validated.targets:
        for artifact in target.artifacts:
            destination = release.joinpath(*artifact.path.parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(artifact.payload)


def assert_secret_removed(run_directory: Path, secret_key: Path) -> None:
    if secret_key.exists():
        raise DeveloperFlasherError("ephemeral secret key still exists before server startup")
    for path in sorted(run_directory.rglob("*")):
        if not path.is_file():
            continue
        value = path.read_bytes()
        if LOCAL_SECRET_KEY_MARKER in value or LOCAL_SECRET_KEY_PLAIN_MARKER in value:
            raise DeveloperFlasherError(
                f"ephemeral secret key material remains before server startup: {path}"
            )


def load_candidate_server() -> ModuleType:
    path = ROOT / "tools" / "release" / "serve-flasher-candidate.py"
    specification = importlib.util.spec_from_file_location("prns_candidate_server", path)
    if specification is None or specification.loader is None:
        raise DeveloperFlasherError(f"could not load candidate server: {path}")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def serve(website_stage: Path, port: int) -> None:
    module = load_candidate_server()
    try:
        server: http.server.ThreadingHTTPServer = module.create_server(website_stage, port)
    except (OSError, ValueError) as error:
        raise DeveloperFlasherError(f"loopback server startup failed: {error}") from error
    address, bound_port = server.server_address
    if address != "127.0.0.1":
        server.server_close()
        raise DeveloperFlasherError(f"local developer flasher bound unexpected address {address}")
    print(
        f"Serving ephemerally signed local firmware at "
        f"http://127.0.0.1:{bound_port}/flash",
        flush=True,
    )
    print("Press Ctrl-C to stop and remove the candidate.", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped local developer flasher.", flush=True)
    finally:
        server.server_close()


def run(selection: Selection) -> None:
    signer_environment = os.environ.copy()
    signer = require_minisign(signer_environment)
    initial_identity = source_identity(ROOT)
    require_unquarantined_source(initial_identity)
    with temporary_run_directory() as run_directory:
        secrets = run_directory / "secrets"
        candidate = run_directory / "candidate"
        website_stage = candidate / "website"
        secrets.mkdir(mode=0o700)
        candidate.mkdir()
        website_stage.mkdir()
        public_key, secret_key, key_id = generate_key(
            signer,
            secrets,
            signer_environment,
        )
        manifest = build_firmware(candidate, initial_identity, selection, key_id)
        validated = verify_manifest_artifacts(
            candidate,
            manifest,
            initial_identity,
            selection,
            key_id,
        )
        build_website(website_stage, initial_identity, selection, public_key)
        channel = write_channel_descriptor(candidate, initial_identity, manifest)
        final_identity = source_identity(ROOT)
        require_unchanged_source(initial_identity, final_identity)
        manifest_signature = sign_and_verify(
            signer,
            manifest,
            secret_key,
            public_key,
            signer_environment,
        )
        channel_signature = sign_and_verify(
            signer,
            channel,
            secret_key,
            public_key,
            signer_environment,
        )
        stage_signed_release(
            website_stage,
            validated,
            manifest,
            manifest_signature,
            channel,
            channel_signature,
            public_key,
        )
        secret_key.unlink()
        assert_secret_removed(run_directory, secret_key)
        print(
            f"Local candidate {initial_identity.version} contains: "
            f"{', '.join(selection.boards)}",
            flush=True,
        )
        serve(website_stage, selection.port)


def terminate(signum: int, frame: object) -> None:
    del signum, frame
    raise KeyboardInterrupt


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        selection = parse_selection(sys.argv[1:] if arguments is None else arguments)
        previous = signal.signal(signal.SIGTERM, terminate)
        try:
            run(selection)
        finally:
            signal.signal(signal.SIGTERM, previous)
        return 0
    except DeveloperFlasherError as error:
        print(f"local developer flasher failed: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("\nLocal developer flasher interrupted; candidate removed.", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
