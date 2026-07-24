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
VECTORS_PATH = ROOT / "prns-host/conformance/host-contract-v1.json"


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def screaming(name):
    return snake(name).upper()


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
    lines.extend(
        [
            "",
            *ts_string_union("CapabilityName", capability["values"]),
            "",
            *ts_string_union("LinkClosedReason", reason["values"]),
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
            "typedef struct PrnsHost PrnsHost;",
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
            "typedef struct PrnsHostOptions {",
            "    size_t struct_size;",
            "    uint32_t required_abi;",
            "    PrnsStringView required_product_version;",
            "    PrnsLimits limits;",
            "} PrnsHostOptions;",
            "",
            "typedef struct PrnsLifecycle {",
            "    size_t struct_size;",
            "    uint64_t revision;",
            "    PrnsLifecyclePhase phase;",
            "    uint32_t reason;",
            "} PrnsLifecycle;",
            "",
            "PRNS_HOST_API PrnsStatus prns_contract_info(PrnsContractInfo *out_info);",
            "PRNS_HOST_API PrnsStatus prns_host_create(const PrnsHostOptions *options, PrnsHost **out_host);",
            "PRNS_HOST_API void prns_host_release(PrnsHost *host);",
            "PRNS_HOST_API PrnsStatus prns_host_lifecycle(const PrnsHost *host, PrnsLifecycle *out_lifecycle);",
            "PRNS_HOST_API PrnsStatus prns_host_stop(PrnsHost *host);",
            "PRNS_HOST_API PrnsStatus prns_host_claim_application_events(PrnsHost *host, PrnsEventStream **out_stream);",
            "PRNS_HOST_API PrnsStatus prns_host_claim_diagnostics(PrnsHost *host, PrnsEventStream **out_stream);",
            "PRNS_HOST_API void prns_event_stream_release(PrnsEventStream *stream);",
            "PRNS_HOST_API PrnsStatus prns_event_stream_next(PrnsEventStream *stream, uint32_t timeout_millis, PrnsEvent **out_event);",
            "PRNS_HOST_API void prns_event_release(PrnsEvent *event);",
            "PRNS_HOST_API uint32_t prns_event_kind(const PrnsEvent *event);",
            "PRNS_HOST_API PrnsStatus prns_event_bytes(const PrnsEvent *event, PrnsEventField field, PrnsByteView *out_value);",
            "PRNS_HOST_API PrnsStatus prns_event_string(const PrnsEvent *event, PrnsEventField field, PrnsStringView *out_value);",
            "PRNS_HOST_API PrnsStatus prns_event_u64(const PrnsEvent *event, PrnsEventField field, uint64_t *out_value);",
            "PRNS_HOST_API PrnsStatus prns_event_u128(const PrnsEvent *event, PrnsEventField field, uint64_t *out_low, uint64_t *out_high);",
            "PRNS_HOST_API PrnsStatus prns_event_resource_stream(PrnsEvent *event, PrnsResourceStream **out_stream);",
            "PRNS_HOST_API void prns_resource_stream_release(PrnsResourceStream *stream);",
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
        VECTORS_PATH: vectors_output(schema),
    }
    for path, content in outputs.items():
        write_or_check(path, content, args.check)


if __name__ == "__main__":
    main()
