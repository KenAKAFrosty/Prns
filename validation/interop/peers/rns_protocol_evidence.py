from __future__ import annotations

import atexit
import json
import socket
import threading
from pathlib import Path


PROTOCOL_EVIDENCE_SCHEMA = 1
PROTOCOL_EVIDENCE_READY = "RNS_PROTOCOL_EVIDENCE_READY "
PROTOCOL_EVIDENCE_FINAL = "RNS_PROTOCOL_EVIDENCE_FINAL "


def protocol_evidence_snapshot() -> dict[str, object]:
    import RNS

    interfaces = []
    for interface in tuple(RNS.Transport.interfaces):
        interfaces.append(
            {
                "name": str(interface),
                "type": type(interface).__name__,
                "protocol_violations": getattr(interface, "protocol_violations", None),
                "ifac_violations": getattr(interface, "ifac_violations", None),
                "packet_filter_hits": getattr(interface, "packet_filter_hits", None),
            }
        )
    return {"schema": PROTOCOL_EVIDENCE_SCHEMA, "interfaces": interfaces}


def render_protocol_evidence() -> str:
    return json.dumps(protocol_evidence_snapshot(), separators=(",", ":"), sort_keys=True)


class ProtocolEvidenceServer:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.port = self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()
        atexit.register(self.report_final)

    def serve(self) -> None:
        while True:
            connection, _address = self.listener.accept()
            with connection:
                connection.sendall(render_protocol_evidence().encode("utf-8") + b"\n")

    def report_final(self) -> None:
        print(PROTOCOL_EVIDENCE_FINAL + render_protocol_evidence(), flush=True)


def start_reference_reticulum(
    *,
    configdir: str | Path,
    loglevel: int | None,
):
    import RNS

    reticulum = RNS.Reticulum(configdir=str(configdir), loglevel=loglevel)
    evidence = ProtocolEvidenceServer()
    print(PROTOCOL_EVIDENCE_READY + str(evidence.port), flush=True)
    return reticulum
