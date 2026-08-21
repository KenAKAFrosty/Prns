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
    if transport == "nrf-serial-dfu" and not parts and not variants:
        nrf_serial_dfu = target.get("nrf_serial_dfu")
        recovery = (
            nrf_serial_dfu.get("recovery")
            if isinstance(nrf_serial_dfu, dict)
            else None
        )
        artifacts = (
            nrf_serial_dfu.get("application") if isinstance(nrf_serial_dfu, dict) else None,
            nrf_serial_dfu.get("init_packet") if isinstance(nrf_serial_dfu, dict) else None,
            recovery.get("artifact") if isinstance(recovery, dict) else None,
        )
        if all(isinstance(artifact, dict) for artifact in artifacts):
            return list(artifacts)
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


def validate_nrf_serial_dfu_recovery_artifact(
    target: dict,
    application: bytes,
    recovery_payload: bytes,
) -> None:
    nrf_serial_dfu = target.get("nrf_serial_dfu")
    if not isinstance(nrf_serial_dfu, dict):
        raise ValueError("Nordic serial DFU metadata is missing")
    compatibility = nrf_serial_dfu.get("compatibility")
    recovery = nrf_serial_dfu.get("recovery")
    if not isinstance(compatibility, dict) or not isinstance(recovery, dict):
        raise ValueError("Nordic serial DFU recovery metadata is malformed")
    application_artifact = nrf_serial_dfu.get("application")
    init_packet_artifact = nrf_serial_dfu.get("init_packet")
    recovery_artifact = recovery.get("artifact")
    if (
        not isinstance(application_artifact, dict)
        or application_artifact.get("kind") != "dfu-application"
        or not isinstance(init_packet_artifact, dict)
        or init_packet_artifact.get("kind") != "dfu-init-packet"
        or not isinstance(recovery_artifact, dict)
        or recovery_artifact.get("kind") != "uf2"
        or recovery.get("mount_label") != "T1000-E"
        or recovery.get("board_id_prefix") != "nrf52840-t1000-e-v1"
    ):
        raise ValueError("Nordic serial DFU recovery artifact identity is unsupported")
    expected_compatibility = {
        "softdevice_family": "s140",
        "softdevice_version": "7.3.0",
        "fwid": "0x0123",
        "application_base": "0x00027000",
        "application_end_exclusive": "0x000ea000",
    }
    if any(
        compatibility.get(field) != value
        for field, value in expected_compatibility.items()
    ) or recovery.get("family_id") != "0xada52840":
        raise ValueError("Nordic serial DFU recovery compatibility is unsupported")
    if not application:
        raise ValueError("Nordic serial DFU application is empty")
    application_base = int(compatibility["application_base"], 16)
    application_end = int(compatibility["application_end_exclusive"], 16)
    family_id = int(recovery["family_id"], 16)
    if not recovery_payload or len(recovery_payload) % UF2_BLOCK_BYTES != 0:
        raise ValueError("recovery UF2 length is not a nonzero multiple of 512 bytes")
    expected_blocks = (len(application) + UF2_PAYLOAD_BYTES - 1) // UF2_PAYLOAD_BYTES
    actual_blocks = len(recovery_payload) // UF2_BLOCK_BYTES
    if actual_blocks != expected_blocks:
        raise ValueError("recovery UF2 block count disagrees with the exact DFU application")
    expected_address = application_base
    for index in range(actual_blocks):
        block = recovery_payload[
            index * UF2_BLOCK_BYTES : (index + 1) * UF2_BLOCK_BYTES
        ]

        def word(offset: int) -> int:
            return int.from_bytes(block[offset : offset + 4], "little")

        if (
            word(0) != UF2_MAGIC_START_ZERO
            or word(4) != UF2_MAGIC_START_ONE
            or word(508) != UF2_MAGIC_END
        ):
            raise ValueError(f"recovery UF2 block {index} has invalid magic")
        if word(8) != UF2_FAMILY_ID_FLAG:
            raise ValueError(f"recovery UF2 block {index} has unsupported flags")
        if word(20) != index or word(24) != actual_blocks:
            raise ValueError(f"recovery UF2 block {index} is reordered or has the wrong count")
        if word(28) != family_id:
            raise ValueError(f"recovery UF2 block {index} has the wrong family ID")
        address = word(12)
        data_bytes = word(16)
        if address != expected_address:
            raise ValueError(f"recovery UF2 block {index} is not at the next application address")
        if data_bytes != UF2_PAYLOAD_BYTES:
            raise ValueError(f"recovery UF2 block {index} has an unsupported payload length")
        expected_address = address + data_bytes
        if expected_address > application_end:
            raise ValueError(f"recovery UF2 block {index} exceeds the application region")
        application_offset = index * UF2_PAYLOAD_BYTES
        application_end_offset = min(
            application_offset + UF2_PAYLOAD_BYTES,
            len(application),
        )
        expected_payload = application[application_offset:application_end_offset]
        block_payload = block[UF2_DATA_OFFSET : UF2_DATA_OFFSET + UF2_PAYLOAD_BYTES]
        if (
            block_payload[: len(expected_payload)] != expected_payload
            or any(block_payload[len(expected_payload) :])
        ):
            raise ValueError(
                f"recovery UF2 block {index} disagrees with the exact DFU application"
            )
        if any(block[UF2_DATA_OFFSET + data_bytes : UF2_DATA_OFFSET + UF2_DATA_BYTES]):
            raise ValueError(f"recovery UF2 block {index} has nonzero payload padding")
