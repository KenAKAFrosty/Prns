from __future__ import annotations


FLASH_MANIFEST_SCHEMA = 3
UF2_BLOCK_BYTES = 512
UF2_DATA_OFFSET = 32
UF2_DATA_BYTES = 476
UF2_PAYLOAD_BYTES = 256
UF2_MAGIC_START_ZERO = 0x0A324655
UF2_MAGIC_START_ONE = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_FAMILY_ID_FLAG = 0x00002000
T_ECHO_APPLICATION_FLASH_END = 0x000C0000
T_ECHO_COMPATIBILITIES = {
    ("s140", "6.1.1", "0x00b6", "0x00026000", "0xada52840"),
    ("s140", "7.3.0", "0x0123", "0x00027000", "0xada52840"),
}


def require_schema(manifest: dict) -> None:
    if manifest.get("schema") != FLASH_MANIFEST_SCHEMA:
        raise ValueError(f"flash manifest must use schema {FLASH_MANIFEST_SCHEMA}")


def target_artifacts(target: dict) -> list[dict]:
    transport = target.get("transport")
    parts = target.get("parts")
    variants = target.get("variants")
    if not isinstance(parts, list) or not isinstance(variants, list):
        raise ValueError("flash manifest target artifact collections are malformed")
    if transport == "esp-serial" and parts and not variants:
        return parts
    if transport == "uf2-mass-storage" and variants and not parts:
        return variants
    raise ValueError("flash manifest target artifacts disagree with its transport")


def validate_uf2_artifact(variant: dict, payload: bytes) -> None:
    compatibility = tuple(
        variant.get(field)
        for field in (
            "softdevice_family",
            "softdevice_version",
            "fwid",
            "application_base",
            "family_id",
        )
    )
    if compatibility not in T_ECHO_COMPATIBILITIES:
        raise ValueError("UF2 compatibility metadata is unsupported")
    if not payload or len(payload) % UF2_BLOCK_BYTES != 0:
        raise ValueError("UF2 length is not a nonzero multiple of 512 bytes")
    application_base = int(variant["application_base"], 16)
    family_id = int(variant["family_id"], 16)
    block_count = len(payload) // UF2_BLOCK_BYTES
    expected_address = application_base
    for index in range(block_count):
        block = payload[index * UF2_BLOCK_BYTES : (index + 1) * UF2_BLOCK_BYTES]

        def word(offset: int) -> int:
            return int.from_bytes(block[offset : offset + 4], "little")

        if (
            word(0) != UF2_MAGIC_START_ZERO
            or word(4) != UF2_MAGIC_START_ONE
            or word(508) != UF2_MAGIC_END
        ):
            raise ValueError(f"UF2 block {index} has invalid magic")
        if word(8) != UF2_FAMILY_ID_FLAG:
            raise ValueError(f"UF2 block {index} has unsupported flags")
        if word(20) != index or word(24) != block_count:
            raise ValueError(f"UF2 block {index} is reordered or has the wrong count")
        if word(28) != family_id:
            raise ValueError(f"UF2 block {index} has the wrong family ID")
        address = word(12)
        data_bytes = word(16)
        if address != expected_address:
            raise ValueError(f"UF2 block {index} is not at the next application address")
        if data_bytes != UF2_PAYLOAD_BYTES:
            raise ValueError(f"UF2 block {index} has an unsupported payload length")
        expected_address = address + data_bytes
        if expected_address > T_ECHO_APPLICATION_FLASH_END:
            raise ValueError(f"UF2 block {index} exceeds the application region")
        if any(block[UF2_DATA_OFFSET + data_bytes : UF2_DATA_OFFSET + UF2_DATA_BYTES]):
            raise ValueError(f"UF2 block {index} has nonzero payload padding")
