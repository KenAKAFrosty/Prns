#!/usr/bin/env python3

import argparse
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCHEMA_PATH = ROOT / "prns-host/schema/host-contract-v1.json"
RUST_PATH = ROOT / "prns-host/core/src/generated.rs"
TS_PATH = ROOT / "prns-js/src/contract.generated.ts"
C_PATH = ROOT / "prns-host/abi/c/include/prns_host.h"
DOTNET_PATH = (
    ROOT
    / "prns-host/bindings/dotnet/src/PersonalRns/Generated/HostContract.g.cs"
)
PYTHON_PATH = (
    ROOT
    / "prns-host/bindings/python/src/personal_rns/generated.py"
)
GO_PATH = ROOT / "prns-host/bindings/go/contract_generated.go"
SWIFT_PATH = (
    ROOT
    / "prns-host/bindings/swift/Sources/PersonalRns/HostContract.generated.swift"
)
SWIFT_C_HEADER_PATH = (
    ROOT
    / "prns-host/bindings/swift/Sources/CPrnsHost/include/prns_host.h"
)
KOTLIN_PATH = (
    ROOT
    / "prns-host/bindings/jvm/src/main/kotlin/io/reticulum/prns/HostContract.generated.kt"
)
JULIA_PATH = (
    ROOT
    / "prns-host/bindings/julia/src/HostContract.generated.jl"
)
VECTORS_PATH = ROOT / "prns-host/conformance/host-contract-v1.json"


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def screaming(name):
    return snake(name).upper()


def lower_first(name):
    return name[0].lower() + name[1:]


def validate(schema):
    if schema["schemaVersion"] != 1:
        raise ValueError("unsupported host contract schema version")
    fixed_names = set()
    for item in schema["fixedBytes"]:
        if item["name"] in fixed_names or item["length"] < 1:
            raise ValueError(f"invalid fixed byte type {item['name']}")
        fixed_names.add(item["name"])
    enum_names = set()
    for enum in schema["enums"]:
        if enum["name"] in enum_names:
            raise ValueError(f"duplicate enum {enum['name']}")
        enum_names.add(enum["name"])
        names = set()
        values = set()
        for value in enum["values"]:
            if value["name"] in names or value["value"] in values:
                raise ValueError(f"duplicate value in {enum['name']}")
            names.add(value["name"])
            values.add(value["value"])
    union_names = set()
    known_types = fixed_names | enum_names | {
        "bytes",
        "string",
        "u8",
        "u64",
        "u128",
        "DestinationName",
        "ResourceStream",
    }
    for union in schema["unions"]:
        if union["name"] in union_names:
            raise ValueError(f"duplicate union {union['name']}")
        union_names.add(union["name"])
        names = set()
        values = set()
        for case in union["cases"]:
            if case["name"] in names or case["value"] in values:
                raise ValueError(f"duplicate case in {union['name']}")
            names.add(case["name"])
            values.add(case["value"])
            field_names = set()
            for field in case["fields"]:
                if field["name"] in field_names:
                    raise ValueError(
                        f"duplicate field {field['name']} in {union['name']}.{case['name']}"
                    )
                if field["type"] not in known_types | union_names | {
                    item["name"] for item in schema["unions"]
                }:
                    raise ValueError(
                        f"unknown type {field['type']} in {union['name']}.{case['name']}"
                    )
                field_names.add(field["name"])
    for union_name, enum_name in (
        ("ApplicationEvent", "ApplicationEventKind"),
        ("DiagnosticEvent", "DiagnosticEventKind"),
    ):
        union = next(item for item in schema["unions"] if item["name"] == union_name)
        enum = next(item for item in schema["enums"] if item["name"] == enum_name)
        union_cases = {
            item["name"]: item["value"] for item in union["cases"]
        }
        enum_values = {
            item["name"]: item["value"] for item in enum["values"]
        }
        if union_cases != enum_values:
            raise ValueError(f"{union_name} and {enum_name} disagree")
    expected_version = schema["productVersion"]
    version_sources = (
        ROOT / "prns-host/core/Cargo.toml",
        ROOT / "prns-host/abi/c/Cargo.toml",
        ROOT / "prns-js/package.json",
        ROOT / "prns-host/bindings/dotnet/src/PersonalRns/PersonalRns.csproj",
        ROOT / "prns-host/bindings/python/pyproject.toml",
    )
    for source in version_sources:
        content = source.read_text()
        if expected_version not in content:
            raise ValueError(
                f"host contract product version disagrees with {source.relative_to(ROOT)}"
            )


def rust_output(schema):
    lines = [
        f"pub const HOST_SCHEMA_VERSION: u32 = {schema['schemaVersion']};",
        f"pub const HOST_SCHEMA_ABI: u32 = {schema['abi']};",
        f'pub const HOST_SCHEMA_PRODUCT_VERSION: &str = "{schema["productVersion"]}";',
    ]
    for item in schema["fixedBytes"]:
        lines.append(
            f"pub const {screaming(item['name'])}_LENGTH: usize = {item['length']};"
        )
    for key, value in schema["limits"].items():
        lines.append(f"pub const BALANCED_{screaming(key)}: usize = {value};")
    for enum in schema["enums"]:
        name = f"Abi{enum['name']}"
        lines.extend(
            [
                "",
                "#[repr(u32)]",
                "#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]",
                f"pub enum {name} {{",
            ]
        )
        for value in enum["values"]:
            lines.append(f"    {value['name']} = {value['value']},")
        lines.extend(
            [
                "}",
                "",
                f"impl TryFrom<u32> for {name} {{",
                "    type Error = ();",
                "",
                "    fn try_from(value: u32) -> Result<Self, Self::Error> {",
                "        match value {",
            ]
        )
        for value in enum["values"]:
            lines.append(
                f"            {value['value']} => Ok(Self::{value['name']}),"
            )
        lines.extend(["            _ => Err(()),", "        }", "    }", "}"])
    return "\n".join(lines) + "\n"


def ts_type(value):
    return {
        "bytes": "Uint8Array",
        "string": "string",
        "u8": "number",
        "u64": "number",
        "u128": "bigint",
    }.get(value, value)


def ts_union(union):
    lines = [f"export type {union['name']} ="]
    last_index = len(union["cases"]) - 1
    for index, case in enumerate(union["cases"]):
        fields = case["fields"]
        terminal = ";" if index == last_index else ""
        if not fields:
            lines.append(f'  | Tag<"{case["name"]}">{terminal}')
            continue
        lines.extend(["  | Tag<", f'      "{case["name"]}",', "      {"])
        for field in fields:
            optional = "?" if field.get("optional", False) else ""
            lines.append(
                f"        readonly {field['name']}{optional}: {ts_type(field['type'])};"
            )
        lines.extend(["      }", f"    >{terminal}"])
    return "\n".join(lines) + "\n"


def ts_string_union(name, values):
    lines = [f"export type {name} ="]
    last_index = len(values) - 1
    for index, value in enumerate(values):
        terminal = ";" if index == last_index else ""
        lines.append(f'  | "{value["name"]}"{terminal}')
    return lines


def ts_output(schema):
    fixed = schema["fixedBytes"]
    lines = [
        'import type { Tag } from "./casework.js";',
        'import type { StreamClaim } from "./async_lanes.js";',
        "",
        "declare const brand: unique symbol;",
        "",
        "type Brand<Name extends string> = { readonly [brand]: Name };",
        "type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;",
        "",
        f"export const HOST_CONTRACT_ABI = {schema['abi']};",
        f'export const PRODUCT_VERSION = "{schema["productVersion"]}";',
    ]
    for item in fixed:
        lines.append(
            f"export const {screaming(item['name'])}_LENGTH = {item['length']};"
        )
    lines.append("")
    for item in fixed:
        lines.append(
            f'export type {item["name"]} = BrandedBytes<"{item["name"]}">;'
        )
    capability = next(item for item in schema["enums"] if item["name"] == "Capability")
    reason = next(
        item for item in schema["enums"] if item["name"] == "LinkClosedReason"
    )
    host_role = next(item for item in schema["enums"] if item["name"] == "HostRole")
    delivery_evidence = next(
        item for item in schema["enums"] if item["name"] == "DeliveryEvidenceKind"
    )
    lines.extend(
        [
            "",
            *ts_string_union("CapabilityName", capability["values"]),
            "",
            *ts_string_union("LinkClosedReason", reason["values"]),
            "",
            *ts_string_union("HostRoleName", host_role["values"]),
            "",
            *ts_string_union("DeliveryEvidenceKind", delivery_evidence["values"]),
            "",
            "export type PrnsLimits = {",
            "  readonly pendingCommands: number;",
            "  readonly applicationEvents: number;",
            "  readonly retainedEventBytes: number;",
            "  readonly diagnostics: number;",
            "};",
            "",
            "export function balancedLimits(): PrnsLimits {",
            "  return {",
            f"    pendingCommands: {schema['limits']['pendingCommands']},",
            f"    applicationEvents: {schema['limits']['applicationEvents']},",
            f"    retainedEventBytes: {schema['limits']['retainedEventBytes']},",
            f"    diagnostics: {schema['limits']['diagnostics']},",
            "  };",
            "}",
            "",
            "export type DestinationName = {",
            "  readonly appName: string;",
            "  readonly aspects: readonly string[];",
            "};",
            "",
            "export type ResourceStream = {",
            "  readonly totalBytes: number;",
            "  claim(): StreamClaim<Uint8Array>;",
            "};",
            "",
        ]
    )
    for union in schema["unions"]:
        lines.append(ts_union(union))
    return "\n".join(lines)


def c_output(schema):
    lines = [
        "#ifndef PRNS_HOST_H",
        "#define PRNS_HOST_H",
        "",
        "#include <stddef.h>",
        "#include <stdint.h>",
        "",
        "#if defined(_WIN32) && defined(PRNS_HOST_BUILD)",
        "#define PRNS_HOST_API __declspec(dllexport)",
        "#elif defined(_WIN32)",
        "#define PRNS_HOST_API __declspec(dllimport)",
        "#else",
        "#define PRNS_HOST_API",
        "#endif",
        "",
        "#if defined(__cplusplus)",
        'extern "C" {',
        "#endif",
        "",
        f"#define PRNS_HOST_CONTRACT_ABI UINT32_C({schema['abi']})",
        f"#define PRNS_HOST_SCHEMA_VERSION UINT32_C({schema['schemaVersion']})",
    ]
    for item in schema["fixedBytes"]:
        lines.append(
            f"#define PRNS_{screaming(item['name'])}_LENGTH UINT32_C({item['length']})"
        )
    for key, value in schema["limits"].items():
        lines.append(f"#define PRNS_BALANCED_{screaming(key)} UINT64_C({value})")
    lines.append("")
    for enum in schema["enums"]:
        c_name = f"Prns{enum['name']}"
        lines.append(f"typedef uint32_t {c_name};")
        for value in enum["values"]:
            lines.append(
                f"#define PRNS_{screaming(enum['name'])}_{screaming(value['name'])} UINT32_C({value['value']})"
            )
        lines.append("")
    lines.extend(
        [
            "/*",
            " * Ownership and lifetime contract:",
            " * - Input byte/string views and configuration arrays are borrowed only for",
            " *   the duration of the call; prns_host_create copies all retained data.",
            " * - Every non-null opaque handle returned through an out parameter has one",
            " *   owner and must be passed exactly once to its matching *_release function.",
            " * - Release and interrupt functions accept NULL and do nothing. Functions",
            " *   with status results reject other required NULL arguments.",
            " * - A release must not race another operation on the same handle. Interrupt",
            " *   may race its matching wait; release only after that wait has returned.",
            " * - UINT32_MAX is the infinite timeout for command and event-stream waits.",
            " * - All exported calls contain Rust panics and report PRNS_STATUS_PANIC where",
            " *   the function has a status result; no Rust unwinding crosses this ABI.",
            " */",
            "",
            "typedef struct PrnsHost PrnsHost;",
            "typedef struct PrnsCommand PrnsCommand;",
            "typedef struct PrnsEventStream PrnsEventStream;",
            "typedef struct PrnsEvent PrnsEvent;",
            "typedef struct PrnsResourceStream PrnsResourceStream;",
            "",
            "typedef struct PrnsByteView {",
            "    const uint8_t *data;",
            "    size_t length;",
            "} PrnsByteView;",
            "",
            "typedef struct PrnsStringView {",
            "    const uint8_t *data;",
            "    size_t length;",
            "} PrnsStringView;",
            "",
            "typedef struct PrnsContractInfo {",
            "    size_t struct_size;",
            "    uint32_t abi;",
            "    uint32_t schema_version;",
            "    PrnsStringView product_version;",
            "} PrnsContractInfo;",
            "",
            "typedef struct PrnsLimits {",
            "    size_t struct_size;",
            "    size_t pending_commands;",
            "    size_t application_events;",
            "    size_t retained_event_bytes;",
            "    size_t diagnostics;",
            "} PrnsLimits;",
            "",
            "typedef struct PrnsIdentityConfig {",
            "    size_t struct_size;",
            "    PrnsIdentityConfigKind kind;",
            "    PrnsByteView secret;",
            "    PrnsStringView path;",
            "} PrnsIdentityConfig;",
            "",
            "typedef struct PrnsDestinationName {",
            "    size_t struct_size;",
            "    PrnsStringView app_name;",
            "    const PrnsStringView *aspects;",
            "    size_t aspect_count;",
            "} PrnsDestinationName;",
            "",
            "typedef struct PrnsDestinationConfig {",
            "    size_t struct_size;",
            "    PrnsDestinationConfigKind kind;",
            "    PrnsDestinationName name;",
            "    PrnsDestinationIdentityConfigKind identity_kind;",
            "    PrnsIdentityConfig dedicated_identity;",
            "    PrnsByteView announce_app_data;",
            "} PrnsDestinationConfig;",
            "",
            "typedef struct PrnsHostOptions {",
            "    size_t struct_size;",
            "    uint32_t required_abi;",
            "    PrnsStringView required_product_version;",
            "    PrnsLimits limits;",
            "    PrnsHostRole role;",
            "    PrnsIdentityConfig identity;",
            "    const PrnsDestinationConfig *destinations;",
            "    size_t destination_count;",
            "    const PrnsCapability *required_capabilities;",
            "    size_t required_capability_count;",
            "} PrnsHostOptions;",
            "",
            "typedef struct PrnsLifecycle {",
            "    size_t struct_size;",
            "    uint64_t revision;",
            "    PrnsLifecyclePhase phase;",
            "    uint32_t reason;",
            "} PrnsLifecycle;",
            "",
            "typedef struct PrnsCommandResult {",
            "    size_t struct_size;",
            "    PrnsCommandOutcomeKind outcome;",
            "    PrnsCommandFailureKind failure;",
            "    PrnsDeliveryEvidenceKind evidence;",
            "    uint64_t rtt_millis;",
            "    PrnsByteView value;",
            "    PrnsStringView detail;",
            "} PrnsCommandResult;",
            "",
            "/* product_version points to process-lifetime static storage. */",
            "PRNS_HOST_API PrnsStatus prns_contract_info(PrnsContractInfo *out_info);",
            "PRNS_HOST_API PrnsStatus prns_host_create(const PrnsHostOptions *options, PrnsHost **out_host);",
            "PRNS_HOST_API void prns_host_release(PrnsHost *host);",
            "PRNS_HOST_API PrnsStatus prns_host_lifecycle(const PrnsHost *host, PrnsLifecycle *out_lifecycle);",
            "/* Returned host views remain valid until prns_host_release. */",
            "PRNS_HOST_API PrnsStatus prns_host_identity_hash(const PrnsHost *host, PrnsByteView *out_hash);",
            "PRNS_HOST_API size_t prns_host_destination_count(const PrnsHost *host);",
            "PRNS_HOST_API PrnsStatus prns_host_destination_hash(const PrnsHost *host, size_t index, PrnsByteView *out_hash);",
            "PRNS_HOST_API PrnsStatus prns_host_announce(PrnsHost *host, PrnsByteView destination, const PrnsByteView *interface_id, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_send_single_packet(PrnsHost *host, PrnsByteView destination, PrnsByteView payload, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_close_link(PrnsHost *host, PrnsByteView link_id, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_attach_tcp_server(PrnsHost *host, PrnsStringView bind, PrnsBitrateKind bitrate_kind, uint64_t bitrate_bps, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_attach_tcp_client(PrnsHost *host, PrnsStringView target, PrnsBitrateKind bitrate_kind, uint64_t bitrate_bps, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_attach_udp(PrnsHost *host, PrnsStringView local, PrnsStringView peer, PrnsBitrateKind bitrate_kind, uint64_t bitrate_bps, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_detach_interface(PrnsHost *host, PrnsByteView interface_id, PrnsCommand **out_command);",
            "PRNS_HOST_API PrnsStatus prns_host_stop(PrnsHost *host);",
            "/* Result views remain valid until prns_command_release. */",
            "PRNS_HOST_API PrnsStatus prns_command_wait(PrnsCommand *command, uint32_t timeout_millis, PrnsCommandResult *out_result);",
            "PRNS_HOST_API void prns_command_interrupt_wait(PrnsCommand *command);",
            "PRNS_HOST_API void prns_command_release(PrnsCommand *command);",
            "PRNS_HOST_API PrnsStatus prns_host_claim_application_events(PrnsHost *host, PrnsEventStream **out_stream);",
            "PRNS_HOST_API PrnsStatus prns_host_claim_diagnostics(PrnsHost *host, PrnsEventStream **out_stream);",
            "PRNS_HOST_API void prns_event_stream_interrupt_wait(PrnsEventStream *stream);",
            "PRNS_HOST_API void prns_event_stream_release(PrnsEventStream *stream);",
            "PRNS_HOST_API PrnsStatus prns_event_stream_next(PrnsEventStream *stream, uint32_t timeout_millis, PrnsEvent **out_event);",
            "PRNS_HOST_API void prns_event_release(PrnsEvent *event);",
            "PRNS_HOST_API uint32_t prns_event_kind(const PrnsEvent *event);",
            "/* Event views remain valid until prns_event_release. */",
            "PRNS_HOST_API PrnsStatus prns_event_bytes(const PrnsEvent *event, PrnsEventField field, PrnsByteView *out_value);",
            "PRNS_HOST_API PrnsStatus prns_event_string(const PrnsEvent *event, PrnsEventField field, PrnsStringView *out_value);",
            "PRNS_HOST_API PrnsStatus prns_event_u64(const PrnsEvent *event, PrnsEventField field, uint64_t *out_value);",
            "PRNS_HOST_API PrnsStatus prns_event_u128(const PrnsEvent *event, PrnsEventField field, uint64_t *out_low, uint64_t *out_high);",
            "/* A resource may be claimed once and remains owned after its event is released. */",
            "PRNS_HOST_API PrnsStatus prns_event_resource_stream(PrnsEvent *event, PrnsResourceStream **out_stream);",
            "PRNS_HOST_API void prns_resource_stream_release(PrnsResourceStream *stream);",
            "/* out_chunk remains valid until the next call or release on this stream. */",
            "PRNS_HOST_API PrnsStatus prns_resource_stream_next(PrnsResourceStream *stream, size_t maximum_bytes, PrnsByteView *out_chunk, uint8_t *out_finished);",
            "",
            "#if defined(__cplusplus)",
            "}",
            "#endif",
            "",
            "#endif",
            "",
        ]
    )
    return "\n".join(lines)


def python_type(value):
    return {
        "bytes": "bytes",
        "string": "str",
        "u8": "int",
        "u64": "int",
        "u128": "int",
        "ResourceStream": "Any",
    }.get(value, value)


def python_output(schema):
    lines = [
        "from __future__ import annotations",
        "",
        "from dataclasses import dataclass",
        "from enum import IntEnum",
        "from typing import Any, TypeAlias",
        "",
        f"HOST_CONTRACT_ABI = {schema['abi']}",
        f"SCHEMA_VERSION = {schema['schemaVersion']}",
        f'PRODUCT_VERSION = "{schema["productVersion"]}"',
    ]
    for item in schema["fixedBytes"]:
        lines.append(f"{screaming(item['name'])}_LENGTH = {item['length']}")
    for key, value in schema["limits"].items():
        lines.append(f"BALANCED_{screaming(key)} = {value}")
    lines.append("")
    for enum in schema["enums"]:
        lines.append(f"class {enum['name']}(IntEnum):")
        for value in enum["values"]:
            lines.append(f"    {screaming(value['name'])} = {value['value']}")
        lines.append("")
    for item in schema["fixedBytes"]:
        if item.get("secret", False):
            lines.extend(
                [
                    f"class {item['name']}:",
                    "    __slots__ = (\"_value\",)",
                    "",
                    "    def __init__(self, value: bytes | bytearray):",
                    "        value = bytearray(value)",
                    f"        if len(value) != {item['length']}:",
                    f'            raise ValueError("{item["name"]} requires exactly {item["length"]} bytes")',
                    "        self._value = value",
                    "",
                    "    @property",
                    "    def value(self) -> bytes:",
                    "        return bytes(self._value)",
                    "",
                    "    def _view(self) -> memoryview:",
                    "        return memoryview(self._value).toreadonly()",
                    "",
                    "    def close(self) -> None:",
                    "        for index in range(len(self._value)):",
                    "            self._value[index] = 0",
                    "",
                    "    def __del__(self):",
                    "        self.close()",
                    "",
                    "    def __enter__(self):",
                    "        return self",
                    "",
                    "    def __exit__(self, _type, _value, _traceback):",
                    "        self.close()",
                    "",
                ]
            )
        else:
            lines.extend(
                [
                    "@dataclass(frozen=True, slots=True)",
                    f"class {item['name']}:",
                    "    value: bytes",
                    "",
                    "    def __post_init__(self):",
                    "        value = bytes(self.value)",
                    f"        if len(value) != {item['length']}:",
                    f'            raise ValueError("{item["name"]} requires exactly {item["length"]} bytes")',
                    '        object.__setattr__(self, "value", value)',
                    "",
                ]
            )
    lines.extend(
        [
            "@dataclass(frozen=True, slots=True)",
            "class DestinationName:",
            "    app_name: str",
            "    aspects: tuple[str, ...]",
            "",
            "    def __post_init__(self):",
            "        if not self.app_name or not self.aspects or any(not value for value in self.aspects):",
            '            raise ValueError("a destination requires a non-empty app name and aspects")',
            "",
        ]
    )
    aliases = []
    for union in schema["unions"]:
        case_names = []
        for case in union["cases"]:
            case_name = f"{union['name']}{case['name']}"
            case_names.append(case_name)
            lines.append("@dataclass(frozen=True, slots=True)")
            lines.append(f"class {case_name}:")
            if not case["fields"]:
                lines.append("    pass")
            else:
                for field in case["fields"]:
                    field_type = python_type(field["type"])
                    if field.get("optional", False):
                        field_type = f"{field_type} | None"
                    lines.append(f"    {snake(field['name'])}: {field_type}")
            lines.append("")
        aliases.append(f"{union['name']}: TypeAlias = {' | '.join(case_names)}")
    lines.extend(aliases)
    lines.append("")
    return "\n".join(lines)


def dotnet_type(value):
    return {
        "bytes": "ReadOnlyMemory<byte>",
        "string": "string",
        "u8": "byte",
        "u64": "ulong",
        "u128": "UInt128",
        "ResourceStream": "ResourceStream",
    }.get(value, value)


def dotnet_output(schema):
    lines = [
        "#nullable enable",
        "",
        "using System.Collections.Immutable;",
        "",
        "namespace PersonalRns;",
        "",
        "public static class HostContract",
        "{",
        f"    public const uint Abi = {schema['abi']};",
        f"    public const uint SchemaVersion = {schema['schemaVersion']};",
        f'    public const string ProductVersion = "{schema["productVersion"]}";',
    ]
    for item in schema["fixedBytes"]:
        lines.append(
            f"    public const int {item['name']}Length = {item['length']};"
        )
    for key, value in schema["limits"].items():
        lines.append(
            f"    public const int Balanced{key[0].upper() + key[1:]} = {value};"
        )
    lines.extend(["}", ""])
    for enum in schema["enums"]:
        lines.append(f"public enum {enum['name']} : uint")
        lines.append("{")
        for value in enum["values"]:
            lines.append(f"    {value['name']} = {value['value']},")
        lines.extend(["}", ""])
    for item in schema["fixedBytes"]:
        if item.get("secret", False):
            lines.extend(
                [
                    f"public sealed class {item['name']} : IDisposable",
                    "{",
                    "    private byte[]? _bytes;",
                    "",
                    f"    public {item['name']}(ReadOnlySpan<byte> bytes)",
                    "    {",
                    f"        if (bytes.Length != HostContract.{item['name']}Length)",
                    "        {",
                    "            throw new ArgumentException(",
                    f'                $"Expected exactly {{HostContract.{item["name"]}Length}} bytes.",',
                    "                nameof(bytes)",
                    "            );",
                    "        }",
                    "        _bytes = bytes.ToArray();",
                    "    }",
                    "",
                    "    public ReadOnlySpan<byte> Span => _bytes ?? throw new ObjectDisposedException(GetType().Name);",
                    "",
                    f"    ~{item['name']}()",
                    "    {",
                    "        Dispose();",
                    "    }",
                    "",
                    "    public void Dispose()",
                    "    {",
                    "        var bytes = Interlocked.Exchange(ref _bytes, null);",
                    "        if (bytes is not null)",
                    "        {",
                    "            System.Security.Cryptography.CryptographicOperations.ZeroMemory(bytes);",
                    "        }",
                    "        GC.SuppressFinalize(this);",
                    "    }",
                    "}",
                    "",
                ]
            )
        else:
            lines.extend(
                [
                    f"public readonly struct {item['name']} : IEquatable<{item['name']}>",
                    "{",
                    f"    private static readonly byte[] Zero = new byte[HostContract.{item['name']}Length];",
                    "    private readonly byte[]? _bytes;",
                    "",
                    f"    public {item['name']}(ReadOnlySpan<byte> bytes)",
                    "    {",
                    f"        if (bytes.Length != HostContract.{item['name']}Length)",
                    "        {",
                    "            throw new ArgumentException(",
                    f'                $"Expected exactly {{HostContract.{item["name"]}Length}} bytes.",',
                    "                nameof(bytes)",
                    "            );",
                    "        }",
                    "        _bytes = bytes.ToArray();",
                    "    }",
                    "",
                    "    public ReadOnlySpan<byte> Span => _bytes ?? Zero;",
                    "",
                    f"    public bool Equals({item['name']} other) => Span.SequenceEqual(other.Span);",
                    "",
                    f"    public override bool Equals(object? value) => value is {item['name']} other && Equals(other);",
                    "",
                    "    public override int GetHashCode()",
                    "    {",
                    "        var hash = new HashCode();",
                    "        foreach (var value in Span)",
                    "        {",
                    "            hash.Add(value);",
                    "        }",
                    "        return hash.ToHashCode();",
                    "    }",
                    "",
                    f"    public static bool operator ==({item['name']} left, {item['name']} right) => left.Equals(right);",
                    f"    public static bool operator !=({item['name']} left, {item['name']} right) => !left.Equals(right);",
                    "}",
                    "",
                ]
            )
    lines.extend(
        [
            "public sealed record DestinationName(string AppName, ImmutableArray<string> Aspects);",
            "",
        ]
    )
    for union in schema["unions"]:
        name = union["name"]
        lines.append(f"public abstract record {name}")
        lines.append("{")
        lines.append(f"    private protected {name}() {{ }}")
        lines.append("")
        for case in union["cases"]:
            params = []
            for field in case["fields"]:
                field_type = dotnet_type(field["type"])
                if field.get("optional", False):
                    field_type += "?"
                field_name = field["name"][0].upper() + field["name"][1:]
                params.append(f"{field_type} {field_name}")
            if not params:
                lines.append(f"    public sealed record {case['name']}() : {name};")
                continue
            lines.append(f"    public sealed record {case['name']}(")
            last_index = len(params) - 1
            for index, parameter in enumerate(params):
                terminal = "" if index == last_index else ","
                lines.append(f"        {parameter}{terminal}")
            lines.append(f"    ) : {name};")
        lines.append("")
        lines.append("    public TResult Match<TResult>(")
        last_index = len(union["cases"]) - 1
        for index, case in enumerate(union["cases"]):
            terminal = "" if index == last_index else ","
            variable = case["name"][0].lower() + case["name"][1:]
            lines.append(
                f"        Func<{name}.{case['name']}, TResult> {variable}{terminal}"
            )
        lines.append("    ) =>")
        lines.append("        this switch")
        lines.append("        {")
        for case in union["cases"]:
            variable = case["name"][0].lower() + case["name"][1:]
            lines.append(
                f"            {case['name']} value => {variable}(value),"
            )
        lines.append('            _ => throw new InvalidOperationException("Unknown contract case."),')
        lines.extend(["        };", "}", ""])
    return "\n".join(lines)


def go_type(value):
    return {
        "bytes": "[]byte",
        "string": "string",
        "u8": "uint8",
        "u64": "uint64",
        "u128": "UInt128",
    }.get(value, value)


def go_output(schema):
    lines = [
        "package prns",
        "",
        "const (",
        f"\tHostContractABI uint32 = {schema['abi']}",
        f"\tHostSchemaVersion uint32 = {schema['schemaVersion']}",
        f'\tProductVersion = "{schema["productVersion"]}"',
        ")",
        "",
    ]
    for item in schema["fixedBytes"]:
        lines.append(f"const {item['name']}Length = {item['length']}")
    for key, value in schema["limits"].items():
        lines.append(f"const Balanced{key[0].upper() + key[1:]} = {value}")
    lines.extend(
        [
            "",
            "type UInt128 struct {",
            "\tLow uint64",
            "\tHigh uint64",
            "}",
            "",
        ]
    )
    for enum in schema["enums"]:
        lines.extend(
            [
                f"type {enum['name']} uint32",
                "",
                "const (",
            ]
        )
        for value in enum["values"]:
            lines.append(
                f"\t{enum['name']}{value['name']} {enum['name']} = {value['value']}"
            )
        lines.extend([")", ""])
    for item in schema["fixedBytes"]:
        lines.append(f"type {item['name']} [{item['name']}Length]byte")
        if item.get("secret", False):
            lines.extend(
                [
                    "",
                    f"func (value *{item['name']}) Close() {{",
                    "\tclear(value[:])",
                    "}",
                ]
            )
        lines.append("")
    lines.extend(
        [
            "type DestinationName struct {",
            "\tAppName string",
            "\tAspects []string",
            "}",
            "",
            "type ResourceStream interface {",
            "\tTotalBytes() uint64",
            "\tNext(maximumBytes int) ([]byte, bool, error)",
            "\tClose() error",
            "}",
            "",
        ]
    )
    for union in schema["unions"]:
        name = union["name"]
        marker = lower_first(name)
        lines.extend(
            [
                f"type {name} interface {{",
                f"\t{marker}()",
                "}",
                "",
            ]
        )
        for case in union["cases"]:
            case_name = f"{name}{case['name']}"
            if not case["fields"]:
                lines.append(f"type {case_name} struct{{}}")
            else:
                lines.append(f"type {case_name} struct {{")
                for field in case["fields"]:
                    field_type = go_type(field["type"])
                    if field.get("optional", False):
                        field_type = f"*{field_type}"
                    lines.append(
                        f"\t{field['name'][0].upper() + field['name'][1:]} {field_type}"
                    )
                lines.append("}")
            lines.extend(
                [
                    "",
                    f"func ({case_name}) {marker}() {{}}",
                    "",
                ]
            )
    return "\n".join(lines)


def swift_type(value):
    return {
        "bytes": "[UInt8]",
        "string": "String",
        "u8": "UInt8",
        "u64": "UInt64",
        "u128": "UInt128",
        "ResourceStream": "any ResourceStream",
    }.get(value, value)


def swift_output(schema):
    lines = [
        "import Foundation",
        "",
        "public enum HostContract {",
        f"    public static let abi: UInt32 = {schema['abi']}",
        f"    public static let schemaVersion: UInt32 = {schema['schemaVersion']}",
        f'    public static let productVersion = "{schema["productVersion"]}"',
    ]
    for item in schema["fixedBytes"]:
        lines.append(
            f"    public static let {lower_first(item['name'])}Length = {item['length']}"
        )
    for key, value in schema["limits"].items():
        lines.append(f"    public static let balanced{key[0].upper() + key[1:]} = {value}")
    lines.extend(["}", ""])
    for enum in schema["enums"]:
        lines.append(f"public enum {enum['name']}: UInt32, Sendable {{")
        for value in enum["values"]:
            lines.append(f"    case {lower_first(value['name'])} = {value['value']}")
        lines.extend(["}", ""])
    for item in schema["fixedBytes"]:
        if item.get("secret", False):
            lines.extend(
                [
                    f"public final class {item['name']}: @unchecked Sendable {{",
                    "    private var storage: [UInt8]",
                    "",
                    "    public init(_ bytes: [UInt8]) throws {",
                    f"        guard bytes.count == HostContract.{lower_first(item['name'])}Length else {{",
                    f'            throw ContractValueError.invalidLength(type: "{item["name"]}", actual: bytes.count)',
                    "        }",
                    "        storage = bytes",
                    "    }",
                    "",
                    "    public func withUnsafeBytes<Result>(",
                    "        _ body: (UnsafeRawBufferPointer) throws -> Result",
                    "    ) rethrows -> Result {",
                    "        try storage.withUnsafeBytes(body)",
                    "    }",
                    "",
                    "    public func close() {",
                    "        _ = storage.withUnsafeMutableBytes { bytes in",
                    "            bytes.initializeMemory(as: UInt8.self, repeating: 0)",
                    "        }",
                    "    }",
                    "",
                    "    deinit {",
                    "        close()",
                    "    }",
                    "}",
                    "",
                ]
            )
        else:
            lines.extend(
                [
                    f"public struct {item['name']}: Hashable, Sendable {{",
                    "    public let bytes: [UInt8]",
                    "",
                    "    public init(_ bytes: [UInt8]) throws {",
                    f"        guard bytes.count == HostContract.{lower_first(item['name'])}Length else {{",
                    f'            throw ContractValueError.invalidLength(type: "{item["name"]}", actual: bytes.count)',
                    "        }",
                    "        self.bytes = bytes",
                    "    }",
                    "}",
                    "",
                ]
            )
    lines.extend(
        [
            "public enum ContractValueError: Error, Equatable {",
            "    case invalidLength(type: String, actual: Int)",
            "}",
            "",
            "public struct DestinationName: Hashable, Sendable {",
            "    public let appName: String",
            "    public let aspects: [String]",
            "",
            "    public init(appName: String, aspects: [String]) {",
            "        self.appName = appName",
            "        self.aspects = aspects",
            "    }",
            "}",
            "",
            "public protocol ResourceStream: AnyObject, AsyncSequence, Sendable",
            "where Element == [UInt8] {",
            "    var totalBytes: UInt64 { get }",
            "    func close()",
            "}",
            "",
        ]
    )
    for union in schema["unions"]:
        lines.append(f"public enum {union['name']}: Sendable {{")
        for case in union["cases"]:
            case_name = lower_first(case["name"])
            fields = []
            for field in case["fields"]:
                field_type = swift_type(field["type"])
                if field.get("optional", False):
                    field_type += "?"
                fields.append(f"{field['name']}: {field_type}")
            if fields:
                lines.append(f"    case {case_name}({', '.join(fields)})")
            else:
                lines.append(f"    case {case_name}")
        lines.extend(["}", ""])
    return "\n".join(lines)


def kotlin_type(value):
    return {
        "bytes": "Bytes",
        "string": "String",
        # Kotlin unsigned values compile to name-mangled JVM methods and
        # synthetic constructors, which makes an otherwise shared Kotlin/Java
        # SDK unusable from Java. Int and Long preserve every ABI bit while
        # producing an ordinary, stable JVM surface for both languages.
        "u8": "Int",
        "u64": "Long",
        "u128": "BigInteger",
    }.get(value, value)


def kotlin_name(name):
    if name == "interface":
        return "`interface`"
    return lower_first(name)


def kotlin_output(schema):
    lines = [
        "package io.reticulum.prns",
        "",
        "import java.math.BigInteger",
        "",
        "object HostContract {",
        f"    const val ABI: Int = {schema['abi']}",
        f"    const val SCHEMA_VERSION: Int = {schema['schemaVersion']}",
        f'    const val PRODUCT_VERSION = "{schema["productVersion"]}"',
    ]
    for item in schema["fixedBytes"]:
        lines.append(f"    const val {screaming(item['name'])}_LENGTH = {item['length']}")
    for key, value in schema["limits"].items():
        lines.append(f"    const val BALANCED_{screaming(key)} = {value}")
    lines.extend(["}", ""])
    for enum in schema["enums"]:
        lines.append(f"enum class {enum['name']}(val rawValue: Int) {{")
        last_index = len(enum["values"]) - 1
        for index, value in enumerate(enum["values"]):
            terminal = ";" if index == last_index else ","
            lines.append(f"    {screaming(value['name'])}({value['value']}){terminal}")
        lines.extend(
            [
                "",
                "    companion object {",
                f"        fun fromRawValue(value: Int): {enum['name']}? = entries.firstOrNull {{ it.rawValue == value }}",
                "    }",
                "}",
                "",
            ]
        )
    for item in schema["fixedBytes"]:
        if item.get("secret", False):
            lines.extend(
                [
                    f"class {item['name']}(bytes: ByteArray) : AutoCloseable {{",
                    "    private val storage = bytes.copyOf()",
                    "",
                    "    init {",
                    f"        require(storage.size == HostContract.{screaming(item['name'])}_LENGTH)",
                    "    }",
                    "",
                    "    fun copyBytes(): ByteArray = storage.copyOf()",
                    "",
                    "    override fun close() {",
                    "        storage.fill(0)",
                    "    }",
                    "}",
                    "",
                ]
            )
        else:
            lines.extend(
                [
                    f"class {item['name']}(bytes: ByteArray) {{",
                    "    private val storage = bytes.copyOf()",
                    "",
                    "    init {",
                    f"        require(storage.size == HostContract.{screaming(item['name'])}_LENGTH)",
                    "    }",
                    "",
                    "    fun copyBytes(): ByteArray = storage.copyOf()",
                    "",
                    f"    override fun equals(other: Any?): Boolean = other is {item['name']} && storage.contentEquals(other.storage)",
                    "    override fun hashCode(): Int = storage.contentHashCode()",
                    "}",
                    "",
                ]
            )
    lines.extend(
        [
            "class Bytes(bytes: ByteArray) {",
            "    private val storage = bytes.copyOf()",
            "",
            "    val size: Int",
            "        get() = storage.size",
            "",
            "    fun copyBytes(): ByteArray = storage.copyOf()",
            "",
            "    override fun equals(other: Any?): Boolean = other is Bytes && storage.contentEquals(other.storage)",
            "    override fun hashCode(): Int = storage.contentHashCode()",
            '    override fun toString(): String = "Bytes(size=$size)"',
            "}",
            "",
            "data class DestinationName(",
            "    val appName: String,",
            "    val aspects: List<String>,",
            ")",
            "",
            "interface ResourceStream : AutoCloseable {",
            "    val totalBytes: Long",
            "    fun next(maximumBytes: Int): ResourceChunk",
            "}",
            "",
            "data class ResourceChunk(val bytes: Bytes, val finished: Boolean)",
            "",
        ]
    )
    for union in schema["unions"]:
        name = union["name"]
        lines.extend([f"sealed interface {name}", ""])
        for case in union["cases"]:
            case_name = f"{name}{case['name']}"
            if not case["fields"]:
                lines.append(f"data object {case_name} : {name}")
            else:
                lines.append(f"data class {case_name}(")
                for index, field in enumerate(case["fields"]):
                    field_type = kotlin_type(field["type"])
                    if field.get("optional", False):
                        field_type += "?"
                    terminal = "," if index < len(case["fields"]) - 1 else ""
                    lines.append(
                        f"    val {kotlin_name(field['name'])}: {field_type}{terminal}"
                    )
                lines.append(f") : {name}")
            lines.append("")
    return "\n".join(lines)


def julia_type(value):
    return {
        "bytes": "Vector{UInt8}",
        "string": "String",
        "u8": "UInt8",
        "u64": "UInt64",
        "u128": "UInt128",
    }.get(value, value)


def julia_name(name):
    result = snake(name)
    if result in {"baremodule", "begin", "break", "catch", "const", "continue",
                  "do", "else", "elseif", "end", "export", "finally", "for",
                  "function", "global", "if", "import", "let", "local", "macro",
                  "module", "quote", "return", "struct", "try", "using", "while"}:
        return f'var"{result}"'
    return result


def julia_output(schema):
    lines = [
        f"const HOST_CONTRACT_ABI = UInt32({schema['abi']})",
        f"const HOST_SCHEMA_VERSION = UInt32({schema['schemaVersion']})",
        f'const PRODUCT_VERSION = "{schema["productVersion"]}"',
    ]
    for item in schema["fixedBytes"]:
        lines.append(f"const {screaming(item['name'])}_LENGTH = {item['length']}")
    for key, value in schema["limits"].items():
        lines.append(f"const BALANCED_{screaming(key)} = {value}")
    lines.append("")
    for enum in schema["enums"]:
        lines.append(f"@enum {enum['name']}::UInt32 begin")
        for value in enum["values"]:
            lines.append(
                f"    {enum['name']}{value['name']} = {value['value']}"
            )
        lines.extend(["end", ""])
    for item in schema["fixedBytes"]:
        if item.get("secret", False):
            lines.extend(
                [
                    f"mutable struct {item['name']}",
                    "    bytes::Vector{UInt8}",
                    "",
                    f"    function {item['name']}(bytes::AbstractVector{{UInt8}})",
                    f'        length(bytes) == {item["length"]} || throw(ArgumentError("{item["name"]} requires {item["length"]} bytes"))',
                    "        value = new(Vector{UInt8}(bytes))",
                    "        finalizer(close, value)",
                    "        value",
                    "    end",
                    "end",
                    "",
                    f"function Base.close(value::{item['name']})",
                    "    fill!(value.bytes, 0x00)",
                    "    nothing",
                    "end",
                    "",
                ]
            )
        else:
            lines.extend(
                [
                    f"struct {item['name']}",
                    f"    bytes::NTuple{{{item['length']},UInt8}}",
                    "",
                    f"    function {item['name']}(bytes)",
                    f'        length(bytes) == {item["length"]} || throw(ArgumentError("{item["name"]} requires {item["length"]} bytes"))',
                    f"        new(Tuple(UInt8(value) for value in bytes)::NTuple{{{item['length']},UInt8}})",
                    "    end",
                    "end",
                    "",
                ]
            )
    lines.extend(
        [
            "struct DestinationName",
            "    app_name::String",
            "    aspects::Vector{String}",
            "end",
            "",
            "abstract type ResourceStream end",
            "",
        ]
    )
    for union in schema["unions"]:
        lines.extend([f"abstract type {union['name']} end", ""])
        for case in union["cases"]:
            case_name = f"{union['name']}{case['name']}"
            lines.append(f"struct {case_name} <: {union['name']}")
            if not case["fields"]:
                lines.append("end")
            else:
                for field in case["fields"]:
                    field_type = julia_type(field["type"])
                    if field.get("optional", False):
                        field_type = f"Union{{Nothing,{field_type}}}"
                    lines.append(f"    {julia_name(field['name'])}::{field_type}")
                lines.append("end")
            lines.append("")
    return "\n".join(lines)


def vectors_output(schema):
    return (
        json.dumps(
            {
                "schemaVersion": schema["schemaVersion"],
                "abi": schema["abi"],
                "productVersion": schema["productVersion"],
                "fixedBytes": {
                    item["name"]: item["length"] for item in schema["fixedBytes"]
                },
                "limits": schema["limits"],
                "enums": {
                    enum["name"]: {
                        value["name"]: value["value"] for value in enum["values"]
                    }
                    for enum in schema["enums"]
                },
                "unions": {
                    union["name"]: {
                        case["name"]: case["value"] for case in union["cases"]
                    }
                    for union in schema["unions"]
                },
                "contractChecks": [
                    {
                        "requiredAbi": schema["abi"],
                        "requiredProductVersion": schema["productVersion"],
                        "outcome": "Compatible",
                    },
                    {
                        "requiredAbi": schema["abi"] + 1,
                        "requiredProductVersion": schema["productVersion"],
                        "outcome": "ContractMismatch",
                    },
                    {
                        "requiredAbi": schema["abi"],
                        "requiredProductVersion": "0.0.0",
                        "outcome": "ContractMismatch",
                    },
                ],
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )


def write_or_check(path, content, check):
    if check:
        if not path.exists() or path.read_text() != content:
            raise ValueError(f"generated host contract is stale: {path.relative_to(ROOT)}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(content)
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    schema = json.loads(SCHEMA_PATH.read_text())
    validate(schema)
    outputs = {
        RUST_PATH: rust_output(schema),
        TS_PATH: ts_output(schema),
        C_PATH: c_output(schema),
        DOTNET_PATH: dotnet_output(schema),
        PYTHON_PATH: python_output(schema),
        GO_PATH: go_output(schema),
        SWIFT_PATH: swift_output(schema),
        SWIFT_C_HEADER_PATH: c_output(schema),
        KOTLIN_PATH: kotlin_output(schema),
        JULIA_PATH: julia_output(schema),
        VECTORS_PATH: vectors_output(schema),
    }
    for path, content in outputs.items():
        write_or_check(path, content, args.check)


if __name__ == "__main__":
    main()
