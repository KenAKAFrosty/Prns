#!/usr/bin/env python3
"""Generate the Python-authoritative LXMF 1.1.0 direct-link corpus.

The generator imports the exact Python LXMF and RNS implementations pinned in
``requirements-lxmf-1.1.0.txt``. It does not import product Rust code. Fixed
identity private keys in this file match public interoperability fixture input
parameters also used by the Precursor oracle; they are behavioral test facts,
not copied implementation source or usable credentials.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import importlib.metadata
import json
import platform
import struct
import tempfile
from pathlib import Path
from typing import Any

import LXMF
import RNS
import RNS.vendor.umsgpack as umsgpack


ROOT = Path(__file__).parents[2]
DEFAULT_OUTPUT = ROOT / "interop" / "vectors" / "lxmf-1.1.0-v1.json"
REQUIREMENTS = ROOT / "interop" / "python" / "requirements-lxmf-1.1.0.txt"

LXMF_VERSION = "1.1.0"
LXMF_REVISION = "795fdaa2b0777c13033787d933d1afc94a2377cb"
RNS_VERSION = "1.5.2"
RNS_REVISION = "ea98db4f53dcf0defc0e71a16e60d28b1229c4e6"
LXMF_MESSAGE_SHA256 = (
    "9a035d03d36e80b615edfb1dbdc44abbbccd672f4a05b0802ad4b98366278e96"
)
LXMF_STAMPER_SHA256 = (
    "eeeba0158546d2e9878ca485ffa4b96dd13ce3e71880d784087c1fdae22538d0"
)
LXMF_CORE_SHA256 = (
    "23b280e47f0690d27dfe469c3117f9928363716260dc85c9266f5d91146b8e90"
)
UMSGPACK_VERSION = "2.7.1"
UMSGPACK_SHA256 = (
    "dd0ea6e77d55eb65193af824f004e0d4334fb11077de64b232556f9759967708"
)
RNS_SOURCE_SHA256 = {
    "Destination.py": "7db80f04e8bac330d1c8a8e57f33c49b7df99798f54c74aae3e2d8e5b3bffbde",
    "Identity.py": "447d6a8406ab0f824c9c6488de1f14434003d21bdfef65d33d79b20d8a2698e3",
    "Link.py": "d1ffccda72963bfe910799b62e363fecdbd45bb5038c81ee857beccdcf3c2da1",
    "Packet.py": "51ce9967598de7bdbf6dfe937c5a1577ae9ddf1b701aef4c54689805a046b7de",
    "Reticulum.py": "e7c8908081132a6ca26593a833a5a41cb1bedaf45713958a1cac870b758f5fc7",
    "Transport.py": "97224d31475741e2d943779ce497853140344f5c9898afcaa9b87ddc937b2340",
}

SOURCE_PRIVATE_KEY = bytes([0x05]) * 32 + bytes([0x06]) * 32
DESTINATION_PRIVATE_KEY = bytes([0x07]) * 32 + bytes([0x08]) * 32
TICKET = bytes(range(0xA0, 0xB0))
TICKET_EXPIRY = 1_782_768_026.0
POW_COST = 8

_LX_MESSAGE_MODULE = importlib.import_module("LXMF.LXMessage")
_LX_STAMPER_MODULE = importlib.import_module("LXMF.LXStamper")
_LXMF_CORE_MODULE = importlib.import_module("LXMF.LXMF")
LXMessage = _LX_MESSAGE_MODULE.LXMessage
LXStamper = _LX_STAMPER_MODULE

_RNS_SOURCE_MODULES = {
    filename: importlib.import_module(f"RNS.{filename.removesuffix('.py')}")
    for filename in RNS_SOURCE_SHA256
}

_RNS_TEMP: tempfile.TemporaryDirectory[str] | None = None
_RNS_INSTANCE: Any = None
_SOURCE_IDENTITY: Any = None
_DESTINATION_IDENTITY: Any = None
_SOURCE_DESTINATION: Any = None
_DESTINATION: Any = None


def _sha256_file(path: str | Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def _direct_url(distribution: str) -> dict[str, object] | None:
    raw = importlib.metadata.distribution(distribution).read_text("direct_url.json")
    return None if raw is None else json.loads(raw)


def authority_provenance() -> dict[str, object]:
    """Return and verify the exact Python compatibility authority."""
    observed = {
        "lxmf_version": LXMF.__version__,
        "rns_version": RNS.__version__,
        "lxmf_message_sha256": _sha256_file(_LX_MESSAGE_MODULE.__file__),
        "lxmf_stamper_sha256": _sha256_file(_LX_STAMPER_MODULE.__file__),
        "lxmf_core_sha256": _sha256_file(_LXMF_CORE_MODULE.__file__),
        "lxmf_direct_url": _direct_url("lxmf"),
        "umsgpack_version": umsgpack.__version__,
        "umsgpack_sha256": _sha256_file(umsgpack.__file__),
        "rns_source_sha256": {
            filename: _sha256_file(module.__file__)
            for filename, module in _RNS_SOURCE_MODULES.items()
        },
        "rns_direct_url": _direct_url("rns"),
    }
    expected = {
        "lxmf_version": LXMF_VERSION,
        "rns_version": RNS_VERSION,
        "lxmf_message_sha256": LXMF_MESSAGE_SHA256,
        "lxmf_stamper_sha256": LXMF_STAMPER_SHA256,
        "lxmf_core_sha256": LXMF_CORE_SHA256,
        "lxmf_direct_url": {
            "url": "https://github.com/markqvist/LXMF.git",
            "vcs_info": {
                "vcs": "git",
                "commit_id": LXMF_REVISION,
                "requested_revision": LXMF_REVISION,
            },
        },
        "umsgpack_version": UMSGPACK_VERSION,
        "umsgpack_sha256": UMSGPACK_SHA256,
        "rns_source_sha256": RNS_SOURCE_SHA256,
        "rns_direct_url": {
            "url": "https://github.com/markqvist/Reticulum.git",
            "vcs_info": {
                "vcs": "git",
                "commit_id": RNS_REVISION,
                "requested_revision": RNS_REVISION,
            },
        },
    }
    if observed != expected:
        raise RuntimeError(
            "LXMF corpus authority mismatch; install "
            "interop/python/requirements-lxmf-1.1.0.txt\n"
            f"expected={expected!r}\nobserved={observed!r}"
        )
    return {
        "implementation": "Python LXMF",
        "version": LXMF_VERSION,
        "repository": "https://github.com/markqvist/LXMF.git",
        "revision": LXMF_REVISION,
        "license": "Reticulum License",
        "lxmf_message_sha256": LXMF_MESSAGE_SHA256,
        "lxmf_stamper_sha256": LXMF_STAMPER_SHA256,
        "lxmf_core_sha256": LXMF_CORE_SHA256,
        "vcs": expected["lxmf_direct_url"],
        "reticulum": {
            "implementation": "Python RNS",
            "version": RNS_VERSION,
            "repository": "https://github.com/markqvist/Reticulum.git",
            "revision": RNS_REVISION,
            "source_sha256": RNS_SOURCE_SHA256,
            "vcs": expected["rns_direct_url"],
        },
        "messagepack": {
            "implementation": "RNS.vendor.umsgpack",
            "version": UMSGPACK_VERSION,
            "source_sha256": UMSGPACK_SHA256,
        },
    }


def _announce_vectors() -> list[dict[str, object]]:
    """Generate shared announce fixtures through the pinned Python helpers."""

    class DeliveryDestination:
        def __init__(self, display_name: str, stamp_cost: int | None):
            self.display_name = display_name
            self.stamp_cost = stamp_cost

    class RouterView:
        def __init__(self, destination_hash: bytes, destination: DeliveryDestination):
            self.delivery_destinations = {destination_hash: destination}

    def current_router_bytes(display_name: str, stamp_cost: int | None = None) -> bytes:
        destination_hash = bytes(16)
        view = RouterView(
            destination_hash,
            DeliveryDestination(display_name, stamp_cost),
        )
        return LXMF.LXMRouter.get_announce_app_data(view, destination_hash)

    fixtures = [
        (
            "personal_hopspot_e290_legacy",
            umsgpack.packb([b"Personal Hopspot E290", None]),
            "legacy_two_item",
            "Personal Hopspot E290 embedded descriptor",
        ),
        (
            "browser_playground_legacy",
            umsgpack.packb([b"Prns Browser Playground", None]),
            "legacy_two_item",
            "browser playground descriptor",
        ),
        (
            "python_lxmf_current",
            current_router_bytes("Pinned Python LXMF"),
            "current_three_item",
            "LXMRouter.get_announce_app_data",
        ),
        (
            "prns_direct_current",
            umsgpack.packb([b"Prns live peer", None, []]),
            "current_three_item",
            "current direct-only no-compression descriptor",
        ),
        (
            "python_display_normalization",
            current_router_bytes(" \x00 Python \x00 peer \t"),
            "current_three_item",
            "LXMRouter display bytes with LXMF helper projection",
        ),
    ]
    records = []
    for name, raw, announce_format, origin in fixtures:
        decoded = umsgpack.unpackb(raw)
        records.append(
            {
                "name": name,
                "origin": origin,
                "format": announce_format,
                "raw_hex": raw.hex(),
                "display_name_hex": (
                    None if decoded[0] is None else decoded[0].hex()
                ),
                "python_display_name": LXMF.display_name_from_app_data(raw),
                "python_stamp_cost": LXMF.stamp_cost_from_app_data(raw),
                "python_compression_support": LXMF.compression_support_from_app_data(
                    raw
                ),
            }
        )
    return records


def _ensure_rns() -> tuple[Any, Any, Any, Any]:
    global _RNS_TEMP
    global _RNS_INSTANCE
    global _SOURCE_IDENTITY
    global _DESTINATION_IDENTITY
    global _SOURCE_DESTINATION
    global _DESTINATION

    if _RNS_INSTANCE is None:
        _RNS_TEMP = tempfile.TemporaryDirectory(prefix="lxmf-corpus-")
        config = Path(_RNS_TEMP.name) / "config"
        config.write_text(
            "[reticulum]\n"
            "  enable_transport = No\n"
            "  share_instance = No\n"
            "  panic_on_interface_error = No\n"
            "[logging]\n"
            "  loglevel = 0\n"
            "[interfaces]\n",
            encoding="utf-8",
        )
        _RNS_INSTANCE = RNS.Reticulum(configdir=_RNS_TEMP.name)

        _SOURCE_IDENTITY = RNS.Identity(create_keys=False)
        _DESTINATION_IDENTITY = RNS.Identity(create_keys=False)
        assert _SOURCE_IDENTITY.load_private_key(SOURCE_PRIVATE_KEY)
        assert _DESTINATION_IDENTITY.load_private_key(DESTINATION_PRIVATE_KEY)
        _SOURCE_DESTINATION = RNS.Destination(
            _SOURCE_IDENTITY,
            RNS.Destination.IN,
            RNS.Destination.SINGLE,
            "lxmf",
            "delivery",
        )
        _DESTINATION = RNS.Destination(
            _DESTINATION_IDENTITY,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "lxmf",
            "delivery",
        )

    return (
        _SOURCE_IDENTITY,
        _DESTINATION_IDENTITY,
        _SOURCE_DESTINATION,
        _DESTINATION,
    )


def _typed_value(value: object) -> dict[str, object]:
    """Represent MessagePack values without losing key or binary types."""
    if value is None:
        return {"type": "nil"}
    if isinstance(value, bool):
        return {"type": "bool", "value": value}
    if isinstance(value, int):
        return {"type": "int", "decimal": str(value)}
    if isinstance(value, float):
        return {
            "type": "f64",
            "bits_hex": struct.pack(">d", value).hex(),
        }
    if isinstance(value, bytes):
        return {"type": "bin", "hex": value.hex()}
    if isinstance(value, str):
        return {"type": "str", "utf8_hex": value.encode("utf-8").hex()}
    if isinstance(value, list):
        return {"type": "array", "items": [_typed_value(item) for item in value]}
    if isinstance(value, dict):
        return {
            "type": "map",
            "entries": [
                {"key": _typed_value(key), "value": _typed_value(item)}
                for key, item in value.items()
            ],
        }
    raise TypeError(f"unsupported fixture value: {type(value).__name__}")


def _method_name(method: int) -> str:
    return {
        LXMessage.UNKNOWN: "unknown",
        LXMessage.OPPORTUNISTIC: "opportunistic",
        LXMessage.DIRECT: "direct",
        LXMessage.PROPAGATED: "propagated",
        LXMessage.PAPER: "paper",
    }[method]


def _representation_name(representation: int) -> str:
    return {
        LXMessage.UNKNOWN: "unknown",
        LXMessage.PACKET: "packet",
        LXMessage.RESOURCE: "resource",
    }[representation]


def _first_valid_pow_stamp(message_id: bytes, cost: int) -> tuple[bytes, int]:
    """Find the first sequential 256-bit stamp accepted by Python LXMF."""
    workblock = LXStamper.stamp_workblock(message_id)
    for candidate_number in range(1 << 24):
        candidate = candidate_number.to_bytes(LXStamper.STAMP_SIZE, "big")
        if LXStamper.stamp_valid(candidate, cost, workblock):
            return candidate, LXStamper.stamp_value(workblock, candidate)
    raise RuntimeError("deterministic LXMF stamp search exhausted")


def _pack_message(
    *,
    timestamp: float,
    title: bytes,
    content: bytes,
    fields: dict[object, object] | None,
    desired_method: int,
    pow_cost: int | None = None,
    outbound_ticket: bytes | None = None,
) -> Any:
    _, _, source_destination, destination = _ensure_rns()
    message = LXMessage(
        destination,
        source_destination,
        content,
        title,
        fields=fields,
        desired_method=desired_method,
        stamp_cost=pow_cost,
    )
    message.timestamp = timestamp
    if outbound_ticket is not None:
        message.outbound_ticket = outbound_ticket
        message.defer_stamp = False
        message.pack()
    elif pow_cost is not None:
        # First freeze the unsigned-stamp four-tuple and its message ID, then
        # deterministically find a valid 32-byte stamp and ask Python LXMF to
        # append it as the fifth element and re-pack/sign the message.
        message.pack()
        stamp, _ = _first_valid_pow_stamp(message.message_id, pow_cost)
        message.packed = None
        message.payload = None
        message.stamp = stamp
        message.defer_stamp = False
        message.pack()
    else:
        message.pack()
    return message


def _ingress_form(message: Any) -> dict[str, object]:
    if message.method == LXMessage.OPPORTUNISTIC:
        return {
            "carrier_event": "destination_data",
            "payload_hex": message.packed[LXMessage.DESTINATION_LENGTH :].hex(),
            "implied_destination_hash_hex": message.destination_hash.hex(),
            "normalization": "prepend_implied_destination_hash",
            "payload_contains_destination_hash": False,
        }
    if message.method == LXMessage.DIRECT and message.representation == LXMessage.PACKET:
        return {
            "carrier_event": "link_data",
            "context": RNS.Packet.NONE,
            "payload_hex": message.packed.hex(),
            "normalization": "identity",
            "payload_contains_destination_hash": True,
        }
    if message.method == LXMessage.DIRECT and message.representation == LXMessage.RESOURCE:
        return {
            "carrier_event": "resource_complete",
            "payload_hex": message.packed.hex(),
            "normalization": "identity",
            "payload_contains_destination_hash": True,
        }
    raise ValueError("first-tranche message selected an unsupported ingress form")


def _message_record(
    name: str,
    description: str,
    message: Any,
    *,
    desired_method: int,
    pow_cost: int | None = None,
    ticket: bytes | None = None,
    ticket_expiry: float | None = None,
    precursor_known_answer: bool = False,
) -> dict[str, object]:
    source_identity, _, _, _ = _ensure_rns()
    wire_payload = message.packed[2 * LXMessage.DESTINATION_LENGTH + LXMessage.SIGNATURE_LENGTH :]
    unpacked_payload = umsgpack.unpackb(wire_payload)
    assert isinstance(unpacked_payload, list) and len(unpacked_payload) in (4, 5)
    payload4 = umsgpack.packb(unpacked_payload[:4])
    fields = unpacked_payload[3]
    hashed_part = message.destination_hash + message.source_hash + payload4
    message_id = RNS.Identity.full_hash(hashed_part)
    signature = message.packed[32:96]
    assert message_id == message.message_id
    assert source_identity.validate(signature, hashed_part + message_id)

    parsed = LXMessage.unpack_from_bytes(message.packed)
    assert parsed.signature_validated
    stamp_record: dict[str, object] | None = None
    if len(unpacked_payload) == 5:
        stamp = unpacked_payload[4]
        if ticket is not None:
            valid = parsed.validate_stamp(31, tickets=[ticket])
            stamp_record = {
                "kind": "ticket",
                "hex": stamp.hex(),
                "length": len(stamp),
                "valid": valid,
                "value": LXMessage.COST_TICKET,
                "ticket_hex": ticket.hex(),
                "ticket_expiry_f64_bits_hex": struct.pack(">d", ticket_expiry).hex(),
            }
        else:
            assert pow_cost is not None
            valid = parsed.validate_stamp(pow_cost)
            workblock = LXStamper.stamp_workblock(message_id)
            stamp_record = {
                "kind": "proof_of_work",
                "hex": stamp.hex(),
                "length": len(stamp),
                "target_cost": pow_cost,
                "valid": valid,
                "value": LXStamper.stamp_value(workblock, stamp),
            }
        assert valid

    return {
        "name": name,
        "description": description,
        "origin": "created by pinned Python LXMF 1.1.0 LXMessage.pack",
        "precursor_python_known_answer_input": precursor_known_answer,
        "desired_method": _method_name(desired_method),
        "actual_method": _method_name(message.method),
        "representation": _representation_name(message.representation),
        "selection_content_size": len(wire_payload)
        - LXMessage.TIMESTAMP_SIZE
        - LXMessage.STRUCT_OVERHEAD,
        "destination_hash_hex": message.destination_hash.hex(),
        "source_hash_hex": message.source_hash.hex(),
        "source_public_key_hex": source_identity.get_public_key().hex(),
        "payload4_hex": payload4.hex(),
        "wire_payload_hex": wire_payload.hex(),
        "fields_msgpack_hex": umsgpack.packb(fields).hex(),
        "decoded": {
            "timestamp_f64_bits_hex": struct.pack(">d", float(unpacked_payload[0])).hex(),
            "title_hex": unpacked_payload[1].hex(),
            "content_hex": unpacked_payload[2].hex(),
            "fields": _typed_value(fields),
        },
        "message_id_hex": message_id.hex(),
        "signature_hex": signature.hex(),
        "full_wire_hex": message.packed.hex(),
        "stamp": stamp_record,
        "ingress": _ingress_form(message),
    }


def _unverified_reason(reason: int | None) -> str | None:
    return {
        None: None,
        LXMessage.SOURCE_UNKNOWN: "source_unknown",
        LXMessage.SIGNATURE_INVALID: "signature_invalid",
    }[reason]


def parse_outcome(wire: bytes) -> dict[str, object]:
    """Record Python LXMF's parse and signature outcome for mutation tests."""
    try:
        parsed = LXMessage.unpack_from_bytes(wire)
    except Exception as error:  # the precise Python exception is corpus data
        return {
            "result": "exception",
            "exception_type": type(error).__name__,
        }
    return {
        "result": "message",
        "message_id_hex": parsed.message_id.hex(),
        "signature_validated": bool(parsed.signature_validated),
        "unverified_reason": _unverified_reason(parsed.unverified_reason),
    }


def _negative_mutations(messages: dict[str, dict[str, object]]) -> list[dict[str, object]]:
    basic = bytes.fromhex(messages["basic_binary"]["full_wire_hex"])
    pow_wire = bytes.fromhex(messages["pow_stamp_32"]["full_wire_hex"])
    ticket_wire = bytes.fromhex(messages["ticket_stamp_16"]["full_wire_hex"])

    def flipped(source: bytes, offset: int, mask: int = 1) -> bytes:
        mutated = bytearray(source)
        mutated[offset] ^= mask
        return bytes(mutated)

    content_offset = basic.index(b"Hello from Python LXMF")
    cases: list[tuple[str, str, bytes, dict[str, object] | None]] = [
        (
            "signature_bit_flip",
            "basic_binary",
            flipped(basic, 32),
            None,
        ),
        (
            "content_bit_flip",
            "basic_binary",
            flipped(basic, content_offset),
            None,
        ),
        (
            "source_hash_bit_flip",
            "basic_binary",
            flipped(basic, LXMessage.DESTINATION_LENGTH),
            None,
        ),
        (
            "truncated_payload",
            "basic_binary",
            basic[:-1],
            None,
        ),
        (
            "pow_stamp_bit_flip",
            "pow_stamp_32",
            flipped(pow_wire, len(pow_wire) - 1),
            {"kind": "proof_of_work", "target_cost": POW_COST, "valid": False},
        ),
        (
            "ticket_stamp_bit_flip",
            "ticket_stamp_16",
            flipped(ticket_wire, len(ticket_wire) - 1),
            {"kind": "ticket", "ticket_hex": TICKET.hex(), "valid": False},
        ),
    ]

    output = []
    for name, based_on, wire, stamp_expectation in cases:
        entry: dict[str, object] = {
            "name": name,
            "based_on": based_on,
            "full_wire_hex": wire.hex(),
            "python_parse": parse_outcome(wire),
        }
        if stamp_expectation is not None:
            parsed = LXMessage.unpack_from_bytes(wire)
            if stamp_expectation["kind"] == "proof_of_work":
                stamp_valid = parsed.validate_stamp(POW_COST)
            else:
                stamp_valid = parsed.validate_stamp(31, tickets=[TICKET])
            assert stamp_valid is stamp_expectation["valid"]
            entry["stamp_validation"] = stamp_expectation
        output.append(entry)
    return output


def _inbound_oracles(messages: dict[str, dict[str, object]]) -> list[dict[str, object]]:
    """Build valid Python-inbound forms that LXMessage.pack does not emit."""
    source_identity, _, _, _ = _ensure_rns()
    basic = messages["basic_binary"]
    destination = bytes.fromhex(basic["destination_hash_hex"])
    source = bytes.fromhex(basic["source_hash_hex"])
    canonical_payload = bytes.fromhex(basic["wire_payload_hex"])
    assert canonical_payload[0] == 0x94

    # Python outbound packing chooses fixarray, but inbound LXMF hashes the
    # exact received payload whenever the decoded array has exactly four
    # values. Array16 therefore needs a fresh signature over its raw bytes.
    received_payload = b"\xdc\x00\x04" + canonical_payload[1:]
    decoded = umsgpack.unpackb(received_payload)
    assert isinstance(decoded, list) and len(decoded) == 4
    canonical_repack = umsgpack.packb(decoded)
    assert canonical_repack == canonical_payload
    assert canonical_repack != received_payload

    hashed_part = destination + source + received_payload
    message_id = RNS.Identity.full_hash(hashed_part)
    signature = source_identity.sign(hashed_part + message_id)
    full_wire = destination + source + signature + received_payload
    python_parse = parse_outcome(full_wire)
    assert python_parse == {
        "result": "message",
        "message_id_hex": message_id.hex(),
        "signature_validated": True,
        "unverified_reason": None,
    }

    return [
        {
            "name": "exact_four_array16_raw_hash",
            "description": (
                "Python-valid noncanonical Array16 envelope proving that an "
                "exact-four inbound payload is hashed byte-for-byte"
            ),
            "origin": "constructed and validated by pinned Python LXMF 1.1.0",
            "hashed_payload_rule": "received_payload_bytes_unchanged",
            "destination_hash_hex": destination.hex(),
            "source_hash_hex": source.hex(),
            "source_public_key_hex": source_identity.get_public_key().hex(),
            "received_payload_hex": received_payload.hex(),
            "canonical_repack_hex": canonical_repack.hex(),
            "message_id_hex": message_id.hex(),
            "signature_hex": signature.hex(),
            "full_wire_hex": full_wire.hex(),
            "python_parse": python_parse,
        }
    ]


def build_vectors() -> dict[str, object]:
    provenance = authority_provenance()
    _ensure_rns()

    rich_fields: dict[object, object] = {
        0x01: [
            None,
            True,
            False,
            -33,
            127,
            128,
            65_536,
            1.5,
            b"\x00\xff",
            "utf8",
        ],
        0x09: {
            b"bin-key": [1, 2],
            "string-key": {"nested": b"\xfe"},
        },
        0x7F: {-1: None},
        "vendor.extension": {"opaque": b"\x00\x01"},
        b"\x00vendor": [False, {"deep": -1}],
    }

    packed_messages = [
        (
            "empty_binary",
            "Empty title and content produce Python content_size -1 and remain opportunistic",
            _pack_message(
                timestamp=1_700_000_004.0,
                title=b"",
                content=b"",
                fields=None,
                desired_method=LXMessage.OPPORTUNISTIC,
            ),
            {"desired_method": LXMessage.OPPORTUNISTIC},
        ),
        (
            "one_byte_content",
            "One content byte produces the adjacent Python content_size zero case",
            _pack_message(
                timestamp=1_700_000_004.0,
                title=b"",
                content=b"x",
                fields=None,
                desired_method=LXMessage.OPPORTUNISTIC,
            ),
            {"desired_method": LXMessage.OPPORTUNISTIC},
        ),
        (
            "basic_binary",
            "Precursor Python known-answer input with binary title/content and no fields or stamp",
            _pack_message(
                timestamp=1_700_000_000.0,
                title=b"Greetings",
                content=b"Hello from Python LXMF",
                fields=None,
                desired_method=LXMessage.OPPORTUNISTIC,
            ),
            {"desired_method": LXMessage.OPPORTUNISTIC, "precursor_known_answer": True},
        ),
        (
            "rich_fields",
            "Invalid-UTF8 binary title/content and nested known/unknown MessagePack fields",
            _pack_message(
                timestamp=1_700_000_001.25,
                title=b"\xff\x00title",
                content=b"\x80\x00rich",
                fields=rich_fields,
                desired_method=LXMessage.OPPORTUNISTIC,
            ),
            {"desired_method": LXMessage.OPPORTUNISTIC},
        ),
        (
            "pow_stamp_32",
            "Direct packet with a deterministic Python-validated 32-byte proof-of-work stamp",
            _pack_message(
                timestamp=1_700_000_002.5,
                title=b"PoW",
                content=b"pow-stamped",
                fields=None,
                desired_method=LXMessage.DIRECT,
                pow_cost=POW_COST,
            ),
            {"desired_method": LXMessage.DIRECT, "pow_cost": POW_COST},
        ),
        (
            "ticket_stamp_16",
            "Direct packet with FIELD_TICKET and a 16-byte ticket-derived stamp",
            _pack_message(
                timestamp=1_700_000_003.75,
                title=b"Ticket",
                content=b"ticket-stamped",
                fields={LXMF.FIELD_TICKET: [TICKET_EXPIRY, TICKET]},
                desired_method=LXMessage.DIRECT,
                outbound_ticket=TICKET,
            ),
            {
                "desired_method": LXMessage.DIRECT,
                "ticket": TICKET,
                "ticket_expiry": TICKET_EXPIRY,
            },
        ),
        (
            "opportunistic_limit_295",
            "Exact Python opportunistic single-packet content-size limit",
            _pack_message(
                timestamp=1_700_000_010.0,
                title=b"",
                content=bytes([0x42]) * 295,
                fields=None,
                desired_method=LXMessage.OPPORTUNISTIC,
            ),
            {"desired_method": LXMessage.OPPORTUNISTIC},
        ),
        (
            "opportunistic_over_296",
            "One byte over the opportunistic limit, causing Python fallback to Direct packet",
            _pack_message(
                timestamp=1_700_000_011.0,
                title=b"",
                content=bytes([0x42]) * 296,
                fields=None,
                desired_method=LXMessage.OPPORTUNISTIC,
            ),
            {"desired_method": LXMessage.OPPORTUNISTIC},
        ),
        (
            "direct_limit_319",
            "Exact Python Direct link-packet content-size limit",
            _pack_message(
                timestamp=1_700_000_012.0,
                title=b"",
                content=bytes([0x43]) * 319,
                fields=None,
                desired_method=LXMessage.DIRECT,
            ),
            {"desired_method": LXMessage.DIRECT},
        ),
        (
            "direct_over_320",
            "One byte over the Direct link-packet limit, selecting Resource",
            _pack_message(
                timestamp=1_700_000_013.0,
                title=b"",
                content=bytes([0x43]) * 320,
                fields=None,
                desired_method=LXMessage.DIRECT,
            ),
            {"desired_method": LXMessage.DIRECT},
        ),
    ]

    messages: dict[str, dict[str, object]] = {}
    ordered_messages = []
    for name, description, message, options in packed_messages:
        record = _message_record(name, description, message, **options)
        messages[name] = record
        ordered_messages.append(record)

    return {
        "schema": 1,
        "protocol": "LXMF",
        "release_lane": "python-lxmf-1.1.0-direct-link",
        "generator": "interop/python/generate_lxmf_1_1_0_vectors.py",
        "generator_source_sha256": _sha256_file(__file__),
        "requirements": "interop/python/requirements-lxmf-1.1.0.txt",
        "requirements_sha256": _sha256_file(REQUIREMENTS),
        "command": (
            "PYTHONPATH=target/interop/lxmf-1.1.0 "
            "python3.13 interop/python/generate_lxmf_1_1_0_vectors.py"
        ),
        "python_version": platform.python_version(),
        "authority": provenance,
        "fixture_identity": {
            "classification": "public deterministic test-only identity",
            "source_public_key_hex": _SOURCE_IDENTITY.get_public_key().hex(),
            "source_destination_hash_hex": _SOURCE_DESTINATION.hash.hex(),
            "destination_public_key_hex": _DESTINATION_IDENTITY.get_public_key().hex(),
            "destination_hash_hex": _DESTINATION.hash.hex(),
            "private_keys_committed_only_in_generator": True,
        },
        "wire_contract": {
            "full_layout": "destination_hash[16] || source_hash[16] || signature[64] || msgpack_payload",
            "payload": "msgpack([timestamp, title, content, fields, optional_stamp])",
            "hashed_payload_exact_four": "received msgpack_payload bytes unchanged when the decoded array has exactly four items",
            "hashed_payload_stamped": "RNS.vendor.umsgpack.packb(decoded_payload[:4]) when the decoded array has more than four items; this corpus emits exactly five",
            "fixture_payload4_hex": "canonical first-four encoding recorded for Python-generated outbound fixtures; it is not evidence that noncanonical exact-four input is re-encoded",
            "message_id": "SHA-256(destination_hash || source_hash || hashed_payload)",
            "signature": "Ed25519(destination_hash || source_hash || hashed_payload || message_id)",
            "opportunistic_ingress": "destination hash implied by RNS DATA; LXMF bytes begin at source hash",
            "direct_packet_ingress": "complete LXMF bytes in LinkData context NONE",
            "direct_resource_ingress": "complete LXMF bytes in ResourceComplete",
        },
        "delivery_constants": {
            "encrypted_packet_max_content": LXMessage.ENCRYPTED_PACKET_MAX_CONTENT,
            "link_packet_max_content": LXMessage.LINK_PACKET_MAX_CONTENT,
            "destination_length": LXMessage.DESTINATION_LENGTH,
            "signature_length": LXMessage.SIGNATURE_LENGTH,
            "ticket_length": LXMessage.TICKET_LENGTH,
            "pow_stamp_length": LXStamper.STAMP_SIZE,
        },
        "announce_vectors": _announce_vectors(),
        "scope": {
            "included": [
                "opportunistic destination-DATA envelope",
                "Direct LinkData envelope",
                "Direct ResourceComplete envelope",
                "binary title and content",
                "rich nested MessagePack fields",
                "message IDs and signatures",
                "32-byte proof-of-work stamp",
                "16-byte ticket-derived stamp and FIELD_TICKET",
                "packet and Resource threshold boundaries",
                "signed negative and zero content-size boundaries",
                "valid noncanonical exact-four inbound raw-byte hashing",
                "negative signature, content, source, truncation, and stamp mutations",
                "legacy/current delivery announces and Python display-name normalization",
            ],
            "deferred": [
                "broader valid noncanonical Python-inbound canonicalization fixtures",
                "propagation encryption and propagation-node envelopes",
                "Paper representation",
                "router persistence, retries, deduplication, and message sync",
                "RNS Resource segmentation and resource-hash derivation",
            ],
        },
        "messages": ordered_messages,
        "inbound_oracles": _inbound_oracles(messages),
        "negative_mutations": _negative_mutations(messages),
    }


def encoded_vectors() -> str:
    return json.dumps(build_vectors(), indent=2, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the committed corpus differs instead of rewriting it",
    )
    args = parser.parse_args()

    generated = encoded_vectors()
    if args.check:
        try:
            committed = args.output.read_text(encoding="utf-8")
        except FileNotFoundError:
            parser.error(f"missing vector file: {args.output}")
        if committed != generated:
            parser.error(f"{args.output} is stale; regenerate it with this script")
        print(f"ok: {args.output} matches Python LXMF {LXMF_VERSION}")
        return 0

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(generated, encoding="utf-8")
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
