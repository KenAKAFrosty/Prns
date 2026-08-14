from __future__ import annotations

import asyncio
import importlib.machinery
import importlib.util
import json
import sys
import types
from pathlib import Path


PACKET_HEX = "0000000102030405060708090a0b0c0d0e0f00c0db7e7d42"


class Interface:
    def __init__(self) -> None:
        self.parent_interface = None
        self.txb = 0
        self.rxb = 0


class Owner:
    def __init__(self) -> None:
        self.inbound_packets: list[str] = []

    def inbound(self, data: bytes, interface: object) -> None:
        self.inbound_packets.append(bytes(data).hex())


class FakeWebSocket:
    def __init__(self, messages: list[bytes | str]) -> None:
        self.messages = messages
        self.sent: list[bytes] = []

    def __aiter__(self):
        return self._messages()

    async def _messages(self):
        for message in self.messages:
            yield message

    async def send(self, data: bytes) -> None:
        self.sent.append(bytes(data))


def install_stubs() -> None:
    rns = types.ModuleType("RNS")
    rns.__path__ = []
    rns.Reticulum = types.SimpleNamespace(HEADER_MINSIZE=19)
    rns.LOG_CRITICAL = 0
    rns.LOG_DEBUG = 0
    rns.LOG_ERROR = 0
    rns.LOG_WARNING = 0
    rns.LOG_VERBOSE = 0
    rns.log = lambda *arguments: None
    rns.panic = lambda: None
    interfaces = types.ModuleType("RNS.Interfaces")
    interfaces.__path__ = []
    interface_module = types.ModuleType("RNS.Interfaces.Interface")
    interface_module.Interface = Interface
    websockets = types.ModuleType("websockets")
    websockets.__spec__ = importlib.machinery.ModuleSpec("websockets", loader=None)
    websockets.exceptions = types.SimpleNamespace(ConnectionClosed=Exception)
    sys.modules.update(
        {
            "RNS": rns,
            "RNS.Interfaces": interfaces,
            "RNS.Interfaces.Interface": interface_module,
            "websockets": websockets,
        }
    )


def load_client(repository: Path):
    path = repository / "src" / "WebSocketClientInterface.py"
    spec = importlib.util.spec_from_file_location("upstream_websocket_client", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.WebSocketClientInterface


async def characterize(repository: Path) -> dict:
    install_stubs()
    client = load_client(repository)
    packet = bytes.fromhex(PACKET_HEX)
    socket = FakeWebSocket([b"short", "text", packet])
    instance = client.__new__(client)
    Interface.__init__(instance)
    instance.websocket = socket
    instance.online = True
    instance.detached = False
    instance.initiator = True
    instance.name = "oracle"
    instance.target_host = "127.0.0.1"
    instance.target_port = 0
    instance.owner = Owner()
    silent_until_outbound = len(socket.sent) == 0
    await instance._send(packet)
    await instance._read_loop()
    return {
        "kind": "runtime",
        "raw": {
            "inbound": instance.owner.inbound_packets,
            "outbound": socket.sent[0].hex(),
            "silent_until_outbound": silent_until_outbound,
        },
    }


def main() -> int:
    repository = Path(sys.argv[1])
    print(json.dumps(asyncio.run(characterize(repository)), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
