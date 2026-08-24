from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

from validation.interop.harness import (
    CommandStream,
    cleanup_temporary_directory,
    decode_command_diagnostic,
    decode_command_output,
    environment,
)


ROOT = Path(__file__).resolve().parents[2]
PEERS = ROOT / "validation" / "interop" / "peers" / "websocket_ecosystem"
INTEGRATION_MANIFEST = ROOT / "validation" / "integration" / "Cargo.toml"
PACKET_HEX = "0000000102030405060708090a0b0c0d0e0f00c0db7e7d42"
KISS_HEX = "c0000000000102030405060708090a0b0c0d0e0f00dbdcdbdd7e7d42c0"
HDLC_HEX = "7e0000000102030405060708090a0b0c0d0e0f00c0db7d5e7d5d427e"


@dataclass(frozen=True)
class PackageLicense:
    path: str
    identifier: str


@dataclass(frozen=True)
class FileLicense:
    path: str
    sha256: str


@dataclass(frozen=True)
class LxmfApplicationPeers:
    echo: str
    sender: str


@dataclass(frozen=True)
class Upstream:
    name: str
    repository: str
    commit: str
    adapter: str | None
    prns_interop_adapter: str | None = None
    prepare_commands: tuple[tuple[str, ...], ...] = ()
    package_licenses: tuple[PackageLicense, ...] = ()
    file_licenses: tuple[FileLicense, ...] = ()
    lxmf_application_peers: LxmfApplicationPeers | None = None


UPSTREAMS = (
    Upstream(
        name="bergie",
        repository="https://github.com/bergie/reticulum-js.git",
        commit="30b93f2d0e2ec2e46f0a88db1d704305c68fad8e",
        adapter="bergie.mjs",
        prns_interop_adapter="bergie_prns.mjs",
        lxmf_application_peers=LxmfApplicationPeers(
            echo="bergie_lxmf/echo.mjs",
            sender="bergie_lxmf/sender.mjs",
        ),
        package_licenses=(
            PackageLicense("packages/core/package.json", "EUPL-1.2"),
            PackageLicense(
                "packages/websocket-server-node/package.json", "EUPL-1.2"
            ),
        ),
    ),
    Upstream(
        name="aerik",
        repository="https://github.com/aerik/reticulum-js.git",
        commit="872d781b1a33c1f4d718a9c05dd0d224f2d790ca",
        adapter="aerik.mjs",
        prepare_commands=(("npm", "ci", "--ignore-scripts", "--omit=dev"),),
        package_licenses=(PackageLicense("package.json", "MIT"),),
        file_licenses=(
            FileLicense(
                "LICENSE",
                "91b33216fc6d2db053d8561279b62fbddfc03198bb3857840d613a9f7a1ce0f9",
            ),
        ),
    ),
    Upstream(
        name="nilu96",
        repository="https://github.com/nilu96/rnsWebsocketInterface.git",
        commit="2a9c214f6a47c75092e7b822184aa3ec3b9f37c0",
        adapter="nilu96.py",
        file_licenses=(
            FileLicense(
                "LICENSE",
                "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4",
            ),
        ),
    ),
    Upstream(
        name="attermann",
        repository="https://github.com/attermann/microReticulum_Firmware.git",
        commit="592c826df31636edaf45b2a0d84f46545041fecb",
        adapter=None,
        file_licenses=(
            FileLicense(
                "LICENSE",
                "3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986",
            ),
        ),
    ),
)


EXPECTED_RUNTIME = {
    "bergie": {
        "kind": "runtime",
        "raw": {
            "inbound": [PACKET_HEX],
            "outbound": PACKET_HEX,
            "silent_until_outbound": True,
        },
        "kiss": {
            "inbound": [PACKET_HEX, PACKET_HEX],
            "outbound": KISS_HEX,
            "silent_until_outbound": True,
        },
    },
    "aerik": {
        "kind": "runtime",
        "hdlc": {
            "inbound": [PACKET_HEX, PACKET_HEX],
            "outbound": HDLC_HEX,
            "silent_until_outbound": True,
        },
    },
    "nilu96": {
        "kind": "runtime",
        "raw": {
            "inbound": [PACKET_HEX],
            "outbound": PACKET_HEX,
            "silent_until_outbound": True,
        },
    },
}


def run(command: list[str], cwd: Path) -> str:
    executable = shutil.which(command[0])
    if executable is None:
        raise RuntimeError(f"required command {command[0]!r} is unavailable")
    completed = subprocess.run(
        [executable, *command[1:]],
        cwd=cwd,
        env=environment({"PYTHONDONTWRITEBYTECODE": "1"}),
        check=False,
        capture_output=True,
        timeout=120,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            json.dumps(
                {
                    "command": command,
                    "cwd": str(cwd),
                    "exit_code": completed.returncode,
                    "stdout": decode_command_diagnostic(completed.stdout),
                    "stderr": decode_command_diagnostic(completed.stderr),
                },
                sort_keys=True,
            )
        )
    return decode_command_output(
        completed.stdout,
        CommandStream.STANDARD_OUTPUT,
        "ecosystem command emitted invalid output",
    ).strip()


def checkout(upstream: Upstream, checkout_root: Path) -> Path:
    destination = checkout_root / upstream.name
    if destination.exists():
        return destination
    destination.mkdir(parents=True)
    run(["git", "init", "--quiet"], destination)
    run(["git", "config", "core.autocrlf", "false"], destination)
    run(["git", "config", "core.eol", "lf"], destination)
    run(["git", "remote", "add", "origin", upstream.repository], destination)
    run(
        ["git", "fetch", "--quiet", "--depth", "1", "origin", upstream.commit],
        destination,
    )
    run(["git", "checkout", "--quiet", "--detach", "FETCH_HEAD"], destination)
    return destination


def verify_checkout(upstream: Upstream, repository: Path) -> None:
    actual_commit = run(["git", "rev-parse", "HEAD"], repository)
    if actual_commit != upstream.commit:
        raise RuntimeError(
            f"{upstream.name} resolved to {actual_commit}, expected {upstream.commit}"
        )

    for package_license in upstream.package_licenses:
        package = json.loads(
            (repository / package_license.path).read_text(encoding="utf-8")
        )
        actual_license = package.get("license")
        if actual_license != package_license.identifier:
            raise RuntimeError(
                f"{upstream.name} {package_license.path} declares {actual_license}, "
                f"expected {package_license.identifier}"
            )

    for file_license in upstream.file_licenses:
        actual_digest = hashlib.sha256(
            (repository / file_license.path).read_bytes()
        ).hexdigest()
        if actual_digest != file_license.sha256:
            raise RuntimeError(
                f"{upstream.name} {file_license.path} has SHA-256 {actual_digest}, "
                f"expected {file_license.sha256}"
            )


def runtime_characterization(upstream: Upstream, repository: Path) -> dict:
    if upstream.adapter is None:
        raise RuntimeError(f"{upstream.name} has no runtime adapter")
    for command in upstream.prepare_commands:
        run(list(command), repository)
    adapter = PEERS / upstream.adapter
    command = (
        [sys.executable, str(adapter), str(repository)]
        if adapter.suffix == ".py"
        else ["node", str(adapter), str(repository)]
    )
    output = run(command, repository)
    result = json.loads(output)
    expected = EXPECTED_RUNTIME[upstream.name]
    if result != expected:
        raise RuntimeError(
            f"{upstream.name} behavior changed: "
            f"expected {json.dumps(expected, sort_keys=True)}, "
            f"received {json.dumps(result, sort_keys=True)}"
        )
    return result


def prns_interoperability(upstream: Upstream, repository: Path) -> dict | None:
    if upstream.prns_interop_adapter is None:
        return None
    adapter = PEERS / upstream.prns_interop_adapter
    for framing in ("raw", "kiss"):
        output = run(
            [
                "cargo",
                "run",
                "--quiet",
                "--manifest-path",
                str(INTEGRATION_MANIFEST),
                "--example",
                "websocket_bergie_peer",
                "--locked",
                "--",
                str(repository),
                str(adapter),
                framing,
            ],
            ROOT,
        )
        expected = f"PASS: bergie {framing} interoperated with Prns auto"
        if output != expected:
            raise RuntimeError(
                f"{upstream.name} {framing} interoperability changed: "
                f"expected {expected!r}, received {output!r}"
            )
    return {
        "kind": "live_websocket",
        "raw": {
            "provisional_raw_received": True,
            "late_evidence_received_by_prns": True,
            "resolved_egress_received": True,
        },
        "kiss": {
            "provisional_raw_discarded": True,
            "late_evidence_received_by_prns": True,
            "resolved_egress_received": True,
        },
    }


def lxmf_application_interoperability(
    upstream: Upstream, repository: Path
) -> dict | None:
    from validation.interop.websocket_bergie_lxmf_e2e import exercise

    peers = upstream.lxmf_application_peers
    if peers is None:
        return None

    return exercise(
        repository,
        PEERS / peers.echo,
        PEERS / peers.sender,
    )


def firmware_source_characterization(repository: Path) -> dict:
    console = (repository / "WebSocketConsole.cpp").read_text(encoding="utf-8")
    server = (repository / "WebSocketServer.cpp").read_text(encoding="utf-8")
    required_console_fragments = (
        "constexpr uint8_t FEND = 0xC0;",
        "serial_fifo_push(data[i]);",
        "g_server->send_binary(g_tx_buf, g_tx_len);",
        "g_tx_buf[0]  = FEND;",
        "if (g_tx_len < TX_BUF_CAP) g_tx_buf[g_tx_len++] = byte;",
    )
    missing = [fragment for fragment in required_console_fragments if fragment not in console]
    if missing:
        raise RuntimeError(f"attermann KISS-over-WebSocket contract changed: {missing}")

    handshake_start = server.index("bool WebSocketServer::finish_handshake()")
    handshake_end = server.index("void WebSocketServer::reset_frame_rx()")
    handshake = server[handshake_start:handshake_end]
    if "Sec-WebSocket-Protocol:" in handshake:
        raise RuntimeError("attermann began selecting a WebSocket subprotocol")
    if "send_binary(" in handshake:
        raise RuntimeError("attermann began sending application data during handshake")

    return {
        "kind": "source_contract",
        "kiss": {
            "complete_frame_per_message": True,
            "silent_after_handshake": True,
        },
        "subprotocol": "not_selected",
    }


def characterize(checkout_root: Path) -> dict:
    results = {}
    for upstream in UPSTREAMS:
        repository = checkout(upstream, checkout_root)
        verify_checkout(upstream, repository)
        result = (
            firmware_source_characterization(repository)
            if upstream.adapter is None
            else runtime_characterization(upstream, repository)
        )
        upstream_result = {
            "commit": upstream.commit,
            "repository": upstream.repository,
            "behavior": result,
        }
        interoperability = prns_interoperability(upstream, repository)
        if interoperability is not None:
            upstream_result["prns_interoperability"] = interoperability
        results[upstream.name] = upstream_result
    return {"schema": 1, "upstreams": results}


def lxmf_application_e2e(checkout_root: Path) -> dict:
    configured = tuple(
        upstream
        for upstream in UPSTREAMS
        if upstream.lxmf_application_peers is not None
    )
    if len(configured) != 1:
        raise RuntimeError(
            f"expected one LXMF application upstream, received {len(configured)}"
        )
    upstream = configured[0]
    repository = checkout(upstream, checkout_root)
    verify_checkout(upstream, repository)
    interoperability = lxmf_application_interoperability(upstream, repository)
    if interoperability is None:
        raise RuntimeError(f"{upstream.name} has no LXMF application peers")
    return {
        "schema": 1,
        "upstreams": {
            upstream.name: {
                "commit": upstream.commit,
                "repository": upstream.repository,
                "interoperability": interoperability,
            }
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--checkout-root", type=Path)
    parser.add_argument("--lxmf-application-e2e", action="store_true")
    arguments = parser.parse_args()
    operation = (
        lxmf_application_e2e
        if arguments.lxmf_application_e2e
        else characterize
    )

    if arguments.checkout_root is not None:
        print(json.dumps(operation(arguments.checkout_root), sort_keys=True))
        return 0

    temporary = tempfile.TemporaryDirectory(prefix="prns-websocket-ecosystem-")
    try:
        print(json.dumps(operation(Path(temporary.name)), sort_keys=True))
    finally:
        cleanup_temporary_directory(temporary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
