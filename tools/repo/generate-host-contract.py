#!/usr/bin/env python3

import argparse
import json
import re
from pathlib import Path

try:
    from host_contract import validate_contract
except ModuleNotFoundError:
    from tools.repo.host_contract import validate_contract


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
    / "prns-host/bindings/jvm/src/main/kotlin/rs/reticulum/prns/HostContract.generated.kt"
)
JULIA_PATH = (
    ROOT
    / "prns-host/bindings/julia/src/HostContract.generated.jl"
)
VECTORS_PATH = ROOT / "prns-host/conformance/host-contract-v1.json"
GO_HOST_PATH = ROOT / "prns-host/bindings/go/convenience.go"
SWIFT_HOST_PATH = ROOT / "prns-host/bindings/swift/Sources/PersonalRns/Host.swift"
KOTLIN_HOST_PATH = (
    ROOT / "prns-host/bindings/jvm/src/main/kotlin/rs/reticulum/prns/Host.kt"
)
KOTLIN_EVENTS_PATH = (
    ROOT / "prns-host/bindings/jvm/src/main/kotlin/rs/reticulum/prns/Events.kt"
)
KOTLIN_UPLOAD_PATH = (
    ROOT / "prns-host/bindings/jvm/src/main/kotlin/rs/reticulum/prns/ResourceUpload.kt"
)
KOTLIN_COMMAND_PATH = (
    ROOT / "prns-host/bindings/jvm/src/main/kotlin/rs/reticulum/prns/Command.kt"
)
JULIA_COMMAND_PATH = ROOT / "prns-host/bindings/julia/src/command.jl"
JULIA_MODULE_PATH = ROOT / "prns-host/bindings/julia/src/PersonalRns.jl"


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def screaming(name):
    return snake(name).upper()


def lower_first(name):
    return name[0].lower() + name[1:]


def swift_identifier(name):
    value = lower_first(name)
    return f"`{value}`" if value in {"internal"} else value


def raw_operations(schema):
    operations = [dict(operation) for operation in schema["operations"]]
    union_name = schema["commandProjection"]["union"]
    command = next(item for item in schema["unions"] if item["name"] == union_name)
    for case in command["cases"]:
        operations.append(
            {
                "name": f"host{case['name']}",
                "kind": "create",
                "receiver": {
                    "type": schema["commandProjection"]["receiver"],
                    "mutable": True,
                },
                "parameters": [
                    {
                        "name": field["name"],
                        "type": field["type"],
                        "passing": "optionalBorrow" if field.get("optional") else "borrow",
                    }
                    for field in case["fields"]
                ],
                "result": {
                    "type": schema["commandProjection"]["result"],
                    "ownership": "owned",
                },
                "status": True,
            }
        )
    names = [operation["name"] for operation in operations]
    if len(names) != len(set(names)):
        raise ValueError("duplicate raw host operation")
    return operations


def raw_operation_parameters(operation):
    parameters = []
    receiver = operation.get("receiver")
    if receiver is not None:
        parameters.append(
            {"name": snake(receiver["type"]), "type": receiver["type"], "receiver": True}
        )
    parameters.extend(operation["parameters"])
    return parameters


def raw_parameter_name(name):
    return {"interface": "interfaceId", "event": "eventValue"}.get(name, name)


def raw_result_type(operation, render_type, generic):
    result = operation.get("result")
    value = render_type("RawUnit") if result is None else render_type(result["type"])
    if result is not None and result["ownership"] == "owned":
        value = generic("RawOwned", value)
    elif result is not None and result["ownership"] == "borrowed":
        value = generic("RawBorrowed", value)
    return generic("RawCallResult", value) if operation["status"] else value


def rust_operation_inventory(schema):
    lines = ["pub const HOST_OPERATION_NAMES: &[&str] = &["]
    for operation in raw_operations(schema):
        lines.append(f'    "{operation["name"]}",')
    lines.append("];")
    return lines


def ts_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["const HOST_OPERATION_NAMES = ["]
    for operation in operations:
        lines.append(f'  "{operation["name"]}",')
    lines.extend(
        [
            "] as const;",
            "",
            "type HostOperationName = (typeof HOST_OPERATION_NAMES)[number];",
            "",
            "type RawUnit = undefined;",
            "type RawOwned<Value> = { readonly value: Value; readonly ownership: \"owned\" };",
            "type RawBorrowed<Value> = { readonly value: Value; readonly ownership: \"borrowed\" };",
            "type RawCallResult<Value> =",
            '  | Tag<"Succeeded", Value>',
            '  | Tag<"Failed", RawStatus>;',
        ]
    )
    raw_names = {item["name"] for item in schema["handles"]} | {
        "ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk",
        "ReadinessCallback", "opaquePointer", "Status", "EventField",
    }
    for name in sorted(raw_names):
        lines.append(f'type Raw{name[0].upper() + name[1:]} = {{ readonly rawType: "{name}" }};')
    lines.extend(["", "interface RawHostProtocol {"])
    for operation in operations:
        def render(value):
            if value == "RawUnit":
                return "RawUnit"
            if value == "size":
                return "number"
            if value in raw_names:
                return f"Raw{value[0].upper() + value[1:]}"
            return ts_type(value)
        parameters = []
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value += " | undefined"
            parameters.append(f"{raw_parameter_name(parameter['name'])}: {value}")
        result = raw_result_type(operation, render, lambda owner, value: f"{owner}<{value}>")
        lines.append(f"  readonly {operation['name']}: ({', '.join(parameters)}) => {result};")
    lines.extend(["}", ""])
    return lines


def python_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["HOST_OPERATION_NAMES: tuple[str, ...] = ("]
    for operation in operations:
        lines.append(f'    "{operation["name"]}",')
    lines.extend([")", "", "RawValue = TypeVar(\"RawValue\")", "", "@dataclass(frozen=True, slots=True)", "class _RawOwned(Generic[RawValue]):", "    value: RawValue", "", "@dataclass(frozen=True, slots=True)", "class _RawBorrowed(Generic[RawValue]):", "    value: RawValue", "", "@dataclass(frozen=True, slots=True)", "class _RawCallSuccess(Generic[RawValue]):", "    value: RawValue", "", "@dataclass(frozen=True, slots=True)", "class _RawCallFailure:", "    error: Status", "", "_RawCallResult: TypeAlias = _RawCallSuccess[RawValue] | _RawCallFailure", "", "class _RawUnit: pass"])
    raw_names = {item["name"] for item in schema["handles"]} | {"ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk", "ReadinessCallback", "opaquePointer"}
    for name in sorted(raw_names):
        lines.extend(["", f"class _Raw{name[0].upper() + name[1:]}: pass"])
    lines.extend(["", "class _RawHostProtocol(Protocol):"])
    for operation in operations:
        def render(value):
            if value == "RawUnit":
                return "_RawUnit"
            if value == "size":
                return "int"
            if value in raw_names:
                return f"_Raw{value[0].upper() + value[1:]}"
            return python_type(value)
        arguments = ""
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value += " | None"
            arguments += f", {snake(parameter['name'])}: {value}"
        result = raw_result_type(operation, render, lambda owner, value: f"_{owner}[{value}]")
        lines.append(f"    def {snake(operation['name'])}(self{arguments}) -> {result}: ...")
    lines.append("")
    return lines


def dotnet_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["internal static class RawHostProtocolContract", "{", "    internal static readonly string[] OperationNames =", "    ["]
    for operation in operations:
        lines.append(f'        "{operation["name"]}",')
    lines.extend(["    ];", "}", "", "internal readonly record struct RawUnit;", "internal readonly record struct RawOwned<T>(T Value);", "internal readonly record struct RawBorrowed<T>(T Value);", "internal abstract record RawCallResult<T>", "{", "    internal sealed record Success(T Value) : RawCallResult<T>;", "    internal sealed record Failure(Status Error) : RawCallResult<T>;", "}"])
    raw_names = {item["name"] for item in schema["handles"]} | {"ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk", "ReadinessCallback", "opaquePointer"}
    for name in sorted(raw_names):
        lines.append(f"internal interface IRaw{name[0].upper() + name[1:]} {{ }}")
    lines.extend(["", "internal interface IRawHostProtocol", "{"])
    for operation in operations:
        def render(value):
            if value == "size":
                return "nuint"
            if value in raw_names:
                return f"IRaw{value[0].upper() + value[1:]}"
            return dotnet_type(value)
        parameters = []
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value += "?"
            parameters.append(f"{value} {raw_parameter_name(parameter['name'])}")
        result = raw_result_type(operation, render, lambda owner, value: f"{owner}<{value}>")
        lines.append(f"    {result} {operation['name'][0].upper() + operation['name'][1:]}({', '.join(parameters)});")
    lines.extend(["}", ""])
    return lines


def go_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["var hostOperationNames = [...]string{"]
    for operation in operations:
        lines.append(f'\t"{operation["name"]}",')
    lines.extend(["}", "", "type rawUnit struct{}", "type rawOwned[T any] struct{ value T }", "type rawBorrowed[T any] struct{ value T }", "type rawCallResult[T any] interface{ rawCallResult() }", "type rawCallSuccess[T any] struct{ value T }", "type rawCallFailure[T any] struct{ error Status }", "func (rawCallSuccess[T]) rawCallResult() {}", "func (rawCallFailure[T]) rawCallResult() {}"])
    raw_names = {item["name"] for item in schema["handles"]} | {"ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk", "ReadinessCallback", "opaquePointer"}
    for name in sorted(raw_names):
        lines.append(f"type raw{name[0].upper() + name[1:]} struct{{}}")
    lines.extend(["", "type rawHostProtocol interface {"])
    for operation in operations:
        def render(value):
            if value == "RawUnit":
                return "rawUnit"
            if value == "size":
                return "uintptr"
            if value in raw_names:
                return f"raw{value[0].upper() + value[1:]}"
            return go_type(value)
        parameters = []
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value = f"*{value}"
            parameters.append(f"{raw_parameter_name(parameter['name'])} {value}")
        result = raw_result_type(operation, render, lambda owner, value: f"{lower_first(owner)}[{value}]")
        lines.append(f"\t{operation['name']}({', '.join(parameters)}) {result}")
    lines.extend(["}", ""])
    return lines


def swift_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["let hostOperationNames = ["]
    for operation in operations:
        lines.append(f'    "{operation["name"]}",')
    lines.extend(["]", "", "struct RawUnit {}", "struct RawOwned<Value> { let value: Value }", "struct RawBorrowed<Value> { let value: Value }", "enum RawCallResult<Value> {", "    case success(Value)", "    case failure(Status)", "}"])
    raw_names = {item["name"] for item in schema["handles"]} | {"ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk", "ReadinessCallback", "opaquePointer"}
    for name in sorted(raw_names):
        lines.append(f"struct Raw{name[0].upper() + name[1:]} {{}}")
    lines.extend(["", "protocol RawHostProtocol {"])
    for operation in operations:
        def render(value):
            if value == "size":
                return "Int"
            if value in raw_names:
                return f"Raw{value[0].upper() + value[1:]}"
            return swift_type(value)
        parameters = []
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value += "?"
            parameters.append(f"_ {raw_parameter_name(parameter['name'])}: {value}")
        result = raw_result_type(operation, render, lambda owner, value: f"{owner}<{value}>")
        lines.append(f"    func {operation['name']}({', '.join(parameters)}) -> {result}")
    lines.extend(["}", ""])
    return lines


def kotlin_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["internal val HOST_OPERATION_NAMES = listOf("]
    for operation in operations:
        lines.append(f'    "{operation["name"]}",')
    lines.extend([")", "", "internal data object RawUnit", "internal data class RawOwned<Value>(val value: Value)", "internal data class RawBorrowed<Value>(val value: Value)", "internal sealed interface RawCallResult<out Value>", "internal data class RawCallSuccess<Value>(val value: Value) : RawCallResult<Value>", "internal data class RawCallFailure(val error: Status) : RawCallResult<Nothing>"])
    raw_names = {item["name"] for item in schema["handles"]} | {"ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk", "ReadinessCallback", "opaquePointer"}
    for name in sorted(raw_names):
        lines.append(f"internal class Raw{name[0].upper() + name[1:]}")
    lines.extend(["", "internal interface RawHostProtocol {"])
    for operation in operations:
        def render(value):
            if value == "size":
                return "Long"
            if value in raw_names:
                return f"Raw{value[0].upper() + value[1:]}"
            return kotlin_type(value)
        parameters = []
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value += "?"
            parameters.append(f"{kotlin_name(parameter['name'])}: {value}")
        result = raw_result_type(operation, render, lambda owner, value: f"{owner}<{value}>")
        lines.append(f"    fun {operation['name']}({', '.join(parameters)}): {result}")
    lines.extend(["}", ""])
    return lines


def julia_raw_protocol(schema):
    operations = raw_operations(schema)
    lines = ["const HOST_OPERATION_NAMES = ("]
    for operation in operations:
        lines.append(f"    :{snake(operation['name'])},")
    lines.extend([")", "", "struct RawUnit end", "struct RawOwned{Value}; value::Value; end", "struct RawBorrowed{Value}; value::Value; end", "abstract type RawCallResult{Value} end", "struct RawCallSuccess{Value} <: RawCallResult{Value}; value::Value; end", "struct RawCallFailure{Value} <: RawCallResult{Value}; error::Status; end"])
    raw_names = {item["name"] for item in schema["handles"]} | {"ContractInfo", "HostOptions", "Lifecycle", "CommandResult", "ResourceChunk", "ReadinessCallback", "opaquePointer"}
    for name in sorted(raw_names):
        lines.append(f"struct Raw{name[0].upper() + name[1:]} end")
    lines.extend(["", "abstract type RawHostProtocol end", ""])
    for operation in operations:
        def render(value):
            if value == "size":
                return "UInt"
            if value in raw_names:
                return f"Raw{value[0].upper() + value[1:]}"
            return julia_type(value)
        parameters = ["protocol::RawHostProtocol"]
        for parameter in raw_operation_parameters(operation):
            value = render(parameter["type"])
            if parameter.get("passing") == "optionalBorrow":
                value = f"Union{{Nothing,{value}}}"
            parameters.append(f"{julia_name(parameter['name'])}::{value}")
        result = raw_result_type(operation, render, lambda owner, value: f"{owner}{{{value}}}")
        lines.extend([f"function {snake(operation['name'])}({', '.join(parameters)})::{result}", "    throw(MethodError(" + snake(operation['name']) + ", (protocol,)))", "end", ""])
    return lines


def validate(schema):
    model = validate_contract(schema)
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
    return model


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
    for scalar in schema["scalars"]:
        if scalar["minimum"] != 0:
            lines.append(
                f"pub const {screaming(scalar['name'])}_MIN: {scalar['storage']} = {scalar['minimum']};"
            )
        lines.append(
            f"pub const {screaming(scalar['name'])}_MAX: {scalar['storage']} = {scalar['maximum']};"
        )
    for key, value in schema["limits"].items():
        lines.append(f"pub const BALANCED_{screaming(key)}: usize = {value};")
    lines.extend(rust_operation_inventory(schema))
    for enum in schema["enums"]:
        name = enum["name"]
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
                f"impl {name} {{",
                "    #[must_use]",
                "    pub const fn contract_name(self) -> &'static str {",
                "        match self {",
            ]
        )
        for value in enum["values"]:
            lines.append(
                f'            Self::{value["name"]} => "{value["name"]}",'
            )
        lines.extend(
            [
                "        }",
                "    }",
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
    lines.extend(
        [
            "",
            "#[cfg(test)]",
            "mod tests {",
            "    use super::*;",
            "",
            "    macro_rules! assert_contract_enum {",
            "        ($enum:ty, [$(($variant:path, $value:literal, $name:literal)),+ $(,)?]) => {{",
            "            $(",
            "                assert_eq!($variant as u32, $value);",
            "                assert_eq!($variant.contract_name(), $name);",
            "                assert_eq!(<$enum>::try_from($value), Ok($variant));",
            "            )+",
            "            assert_eq!(<$enum>::try_from(u32::MAX), Err(()));",
            "        }};",
            "    }",
        ]
    )
    for enum in schema["enums"]:
        name = enum["name"]
        lines.extend(
            [
                "",
                "    #[rustfmt::skip]",
                "    #[test]",
                f"    fn {snake(name)}_values_match_the_contract() {{",
            ]
        )
        lines.append(f"        assert_contract_enum!({name}, [")
        for value in enum["values"]:
            lines.append(
                f'            ({name}::{value["name"]}, {value["value"]}, "{value["name"]}"),'
            )
        lines.extend(["        ]);", "    }"])
    lines.append("}")
    return "\n".join(lines) + "\n"


def ts_type(value):
    if isinstance(value, dict):
        return f"readonly {ts_type(value['array'])}[]"
    return {
        "bytes": "Uint8Array",
        "bool": "boolean",
        "string": "string",
        "i16": "number",
        "i64": "bigint",
        "u8": "number",
        "u16": "number",
        "u32": "number",
        "safeInt": "number",
        "safeUint": "number",
        "u64": "bigint",
        "u128": "bigint",
        "Capability": "CapabilityName",
        "HostRole": "HostRoleName",
    }.get(value, value)


def ts_record(record):
    lines = [f"export type {record['name']} = {{"]
    for field in record["fields"]:
        optional = "?" if field.get("optional", False) else ""
        lines.append(
            f"  readonly {field['name']}{optional}: {ts_type(field['type'])};"
        )
    lines.extend(["};", ""])
    return lines


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
    inventory = f"{screaming(name)}_VALUES"
    lines.extend(
        [
            "",
            f"export const {inventory}: readonly {name}[] = Object.freeze([",
        ]
    )
    for value in values:
        lines.append(f'  "{value["name"]}",')
    lines.extend(
        [
            "]);",
            "",
            f"export function is{name}(value: unknown): value is {name} {{",
            f"  return typeof value === \"string\" && ({inventory} as readonly string[]).includes(value);",
            "}",
        ]
    )
    return lines


def ts_numeric_inventory(name, values):
    inventory = f"{screaming(name)}_CODES"
    lines = [f"export const {inventory} = Object.freeze({{"]
    for value in values:
        lines.append(f'  {value["name"]}: {value["value"]},')
    lines.extend(
        [
            "} as const);",
            "",
            f"export type {name}Code = (typeof {inventory})[keyof typeof {inventory}];",
        ]
    )
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
        f"export const HOST_SCHEMA_VERSION = {schema['schemaVersion']};",
        f'export const PRODUCT_VERSION = "{schema["productVersion"]}";',
    ]
    for item in fixed:
        lines.append(
            f"export const {screaming(item['name'])}_LENGTH = {item['length']};"
        )
    for scalar in schema["scalars"]:
        if scalar["minimum"] != 0:
            lines.append(
                f"export const {screaming(scalar['name'])}_MIN = {scalar['minimum']};"
            )
        lines.append(
            f"export const {screaming(scalar['name'])}_MAX = {scalar['maximum']};"
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
    request_policy = next(
        item for item in schema["enums"] if item["name"] == "RequestPolicy"
    )
    persistence_flush_cause = next(
        item
        for item in schema["enums"]
        if item["name"] == "PersistenceFlushCause"
    )
    persistence_flush_target = next(
        item
        for item in schema["enums"]
        if item["name"] == "PersistenceFlushTarget"
    )
    additional_ts_enums = [
        next(item for item in schema["enums"] if item["name"] == name)
        for name in (
            "BackendKind",
            "InterfaceKind",
            "InterfaceMode",
            "WebSocketFramingSelection",
            "InterfaceHealth",
            "DiscoveryScope",
            "MulticastAddressType",
            "SerialDataBits",
            "SerialParity",
            "SerialStopBits",
        )
    ]
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
            *ts_string_union("RequestPolicy", request_policy["values"]),
            "",
            *ts_string_union(
                "PersistenceFlushCause", persistence_flush_cause["values"]
            ),
            "",
            *ts_string_union(
                "PersistenceFlushTarget", persistence_flush_target["values"]
            ),
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
        ]
    )
    for enum in additional_ts_enums:
        lines.extend([*ts_string_union(enum["name"], enum["values"]), ""])
    for name in ("ApplicationEventKind", "DiagnosticEventKind", "EventField"):
        enum = next(item for item in schema["enums"] if item["name"] == name)
        lines.extend([*ts_numeric_inventory(enum["name"], enum["values"]), ""])
    for record in schema.get("records", []):
        lines.extend(ts_record(record))
    lines.extend(
        [
            "export type ResourceStream = {",
            "  readonly totalBytes: bigint;",
            "  claim(): StreamClaim<Uint8Array>;",
            "};",
            "",
        ]
    )
    for union in schema["unions"]:
        lines.append(ts_union(union))
    lines.extend(ts_raw_protocol(schema))
    return "\n".join(lines)


def c_type(value, fixed_types=frozenset()):
    if value in fixed_types:
        return "PrnsByteView"
    return {
        "bytes": "PrnsByteView",
        "bool": "uint8_t",
        "string": "PrnsStringView",
        "i16": "int16_t",
        "i64": "int64_t",
        "u8": "uint8_t",
        "u16": "uint16_t",
        "u32": "uint32_t",
        "safeInt": "int64_t",
        "safeUint": "uint64_t",
        "u64": "uint64_t",
        "u128": "PrnsUInt128",
    }.get(value, f"Prns{value}")


def c_record(record, fixed_types):
    lines = [f"typedef struct Prns{record['name']} {{", "    size_t struct_size;"]
    for field in record["fields"]:
        field_name = snake(field["name"])
        if field.get("optional", False):
            lines.append(f"    uint8_t has_{field_name};")
        if isinstance(field["type"], dict):
            item_type = c_type(field["type"]["array"], fixed_types)
            if field_name.endswith("ies"):
                count_name = f"{field_name[:-3]}y"
            elif field_name.endswith("s"):
                count_name = field_name[:-1]
            else:
                count_name = field_name
            lines.append(f"    const {item_type} *{field_name};")
            lines.append(f"    size_t {count_name}_count;")
        else:
            lines.append(f"    {c_type(field['type'], fixed_types)} {field_name};")
    lines.extend([f"}} Prns{record['name']};", ""])
    return lines


def c_command_parameters(field, schema, fixed_types):
    name = snake(field["name"])
    type_name = field["type"]
    if name == "interface" and type_name == "InterfaceId":
        name = "interface_id"
    optional = field.get("optional", False)
    if type_name == "Bitrate":
        return [
            f"PrnsBitrateKind {name}_kind",
            f"uint64_t {name}_bps",
        ]
    if type_name == "ResponseTimeout":
        return [
            f"PrnsResponseTimeoutKind {name}_kind",
            f"uint64_t {name}_millis",
        ]
    if type_name == "ResourceCompression":
        return [f"PrnsResourceCompressionKind {name}_kind"]
    if type_name == "ResourceStrategy":
        return [
            f"PrnsResourceStrategyKind {name}_kind",
            "uint64_t maximum_uncompressed_bytes",
            "uint8_t accept_compressed",
        ]
    if type_name == "InterfaceConfig":
        return [f"const PrnsInterfaceConfig *{name}"]
    c_name = c_type(type_name, fixed_types)
    if optional:
        return [f"const {c_name} *{name}"]
    return [f"{c_name} {name}"]


def c_command_declarations(schema):
    fixed_types = {item["name"] for item in schema["fixedBytes"]}
    union_name = schema["commandProjection"]["union"]
    command = next(item for item in schema["unions"] if item["name"] == union_name)
    prefix = schema["commandProjection"]["cPrefix"]
    lines = []
    for case in command["cases"]:
        parameters = ["PrnsHost *host"]
        for field in case["fields"]:
            parameters.extend(c_command_parameters(field, schema, fixed_types))
        parameters.append("PrnsIssuedCommand **out_command")
        lines.append(
            f"PRNS_HOST_API PrnsStatus {prefix}{snake(case['name'])}({', '.join(parameters)});"
        )
    return lines


def c_operation_parameter(parameter, fixed_types):
    type_name = parameter["type"]
    name = snake(parameter["name"])
    passing = parameter["passing"]
    if type_name == "HostOptions":
        return [f"const PrnsHostOptions *{name}"]
    if type_name == "ReadinessCallback":
        return [f"PrnsReadinessCallback {name}"]
    if type_name == "opaquePointer":
        return [f"void *{name}"]
    if type_name == "size":
        return [f"size_t {name}"]
    if type_name == "ResourceCompression":
        return [f"PrnsResourceCompressionKind {name}_kind"]
    if type_name == "Bitrate":
        return [f"PrnsBitrateKind {name}_kind", f"uint64_t {name}_bps"]
    c_name = c_type(type_name, fixed_types)
    if passing == "optionalBorrow":
        return [f"const {c_name} *{name}"]
    return [f"{c_name} {name}"]


def c_operation_result(result):
    if result is None:
        return []
    type_name = result["type"]
    ownership = result["ownership"]
    if ownership == "owned":
        return [f"Prns{type_name} **out_value"]
    if type_name == "u128":
        return ["uint64_t *out_low", "uint64_t *out_high"]
    if type_name == "ResourceChunk":
        return ["PrnsByteView *out_chunk", "uint8_t *out_finished"]
    mapping = {
        "ContractInfo": "PrnsContractInfo",
        "BackendInfo": "PrnsBackendInfo",
        "Lifecycle": "PrnsLifecycle",
        "HostSnapshot": "PrnsHostSnapshot",
        "CommandResult": "PrnsCommandResult",
        "bytes": "PrnsByteView",
        "string": "PrnsStringView",
        "bool": "uint8_t",
        "u64": "uint64_t",
    }
    return [f"{mapping[type_name]} *out_value"]


def c_operation_declaration(operation, handles, fixed_types):
    receiver = operation.get("receiver")
    parameters = []
    if receiver is not None:
        qualifier = "" if receiver["mutable"] else "const "
        parameters.append(
            f"{qualifier}Prns{receiver['type']} *{snake(receiver['type'])}"
        )
    for parameter in operation["parameters"]:
        parameters.extend(c_operation_parameter(parameter, fixed_types))
    if operation["status"]:
        parameters.extend(c_operation_result(operation.get("result")))
    if not parameters:
        parameters = ["void"]
    if operation["status"]:
        return_type = "PrnsStatus"
    elif operation.get("result") is None:
        return_type = "void"
    else:
        return_type = {
            "size": "size_t",
            "u32": "uint32_t",
        }[operation["result"]["type"]]
    return (
        f"PRNS_HOST_API {return_type} prns_{snake(operation['name'])}"
        f"({', '.join(parameters)});"
    )


def c_operation_declarations(schema):
    handles = {item["name"] for item in schema["handles"]}
    fixed_types = {item["name"] for item in schema["fixedBytes"]}
    return [
        c_operation_declaration(operation, handles, fixed_types)
        for operation in schema["operations"]
    ]


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
    for scalar in schema["scalars"]:
        macro = "INT64_C" if scalar["storage"] == "i64" else "UINT64_C"
        if scalar["minimum"] != 0:
            minimum = (
                f"(-{macro}({-scalar['minimum']}))"
                if scalar["minimum"] < 0
                else f"{macro}({scalar['minimum']})"
            )
            lines.append(
                f"#define PRNS_{screaming(scalar['name'])}_MIN {minimum}"
            )
        lines.append(
            f"#define PRNS_{screaming(scalar['name'])}_MAX {macro}({scalar['maximum']})"
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
            " * - UINT32_MAX is the infinite timeout for command, event-stream, and",
            " *   supplied-Pipe request waits.",
            " * - Supplied-Pipe readiness is only a wake hint. Consumers pull an owned",
            " *   open-request handle, then provide or decline it exactly once.",
            " * - A successful descriptor-provide call consumes every non-negative",
            " *   descriptor, including one rejected because closure won a race.",
            " * - All exported calls contain Rust panics and report PRNS_STATUS_PANIC where",
            " *   the function has a status result; no Rust unwinding crosses this ABI.",
            " */",
            "",
        ]
    )
    for handle in schema["handles"]:
        lines.append(f"typedef struct Prns{handle['name']} Prns{handle['name']};")
    lines.extend(
        [
            "typedef void (*PrnsReadinessCallback)(void *context);",
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
            "typedef struct PrnsPersistenceConfig {",
            "    size_t struct_size;",
            "    PrnsPersistenceConfigKind kind;",
            "    PrnsStringView path;",
            "} PrnsPersistenceConfig;",
            "",
        ]
    )
    fixed_types = {item["name"] for item in schema["fixedBytes"]}
    for record in schema.get("records", []):
        lines.extend(c_record(record, fixed_types))
    lines.extend(
        [
            "typedef struct PrnsInterfaceConfig {",
            "    size_t struct_size;",
            "    PrnsInterfaceKind kind;",
            "    uint8_t has_group_id;",
            "    PrnsStringView group_id;",
            "    uint8_t has_discovery_scope;",
            "    PrnsDiscoveryScope discovery_scope;",
            "    uint8_t has_discovery_port;",
            "    uint16_t discovery_port;",
            "    uint8_t has_data_port;",
            "    uint16_t data_port;",
            "    const PrnsStringView *devices;",
            "    size_t device_count;",
            "    const PrnsStringView *ignored_devices;",
            "    size_t ignored_device_count;",
            "    uint8_t has_multicast_address_type;",
            "    PrnsMulticastAddressType multicast_address_type;",
            "    PrnsStringView target;",
            "    PrnsStringView bind;",
            "    PrnsStringView local;",
            "    PrnsStringView peer;",
            "    PrnsBitrateKind bitrate_kind;",
            "    uint64_t bitrate_bps;",
            "    PrnsStringView port;",
            "    PrnsSerialLineConfig line;",
            "    uint8_t flow_control;",
            "    uint32_t preamble_millis;",
            "    uint32_t transmit_tail_millis;",
            "    uint8_t persistence;",
            "    uint32_t slot_time_millis;",
            "    uint8_t has_station_callsign;",
            "    PrnsStringView station_callsign;",
            "    uint8_t has_station_interval_seconds;",
            "    uint64_t station_interval_seconds;",
            "    PrnsStringView callsign;",
            "    uint8_t ssid;",
            "    PrnsRNodeRadioConfig radio;",
            "    uint8_t has_airtime_limit_short_centi_percent;",
            "    uint16_t airtime_limit_short_centi_percent;",
            "    uint8_t has_airtime_limit_long_centi_percent;",
            "    uint16_t airtime_limit_long_centi_percent;",
            "    const PrnsMultiRNodeMemberConfig *members;",
            "    size_t member_count;",
            "    const PrnsStringView *command;",
            "    size_t command_count;",
            "    uint64_t respawn_delay_millis;",
            "    const PrnsStringView *peers;",
            "    size_t peer_count;",
            "    uint8_t connectable;",
            "    PrnsStringView url;",
            "    PrnsWebSocketFramingSelection websocket_framing_selection;",
            "} PrnsInterfaceConfig;",
            "",
            "typedef struct PrnsDestinationConfig {",
            "    size_t struct_size;",
            "    PrnsDestinationConfigKind kind;",
            "    PrnsDestinationName name;",
            "    PrnsDestinationIdentityConfigKind identity_kind;",
            "    PrnsIdentityConfig dedicated_identity;",
            "    PrnsByteView announce_app_data;",
            "    const PrnsRequestHandlerConfig *request_handlers;",
            "    size_t request_handler_count;",
            "    uint8_t has_maximum_request_bytes;",
            "    uint64_t maximum_request_bytes;",
            "} PrnsDestinationConfig;",
            "",
            "typedef struct PrnsHostOptions {",
            "    size_t struct_size;",
            "    uint32_t required_abi;",
            "    uint32_t required_schema_version;",
            "    PrnsStringView required_product_version;",
            "    PrnsLimits limits;",
            "    PrnsHostRole role;",
            "    PrnsIdentityConfig identity;",
            "    const PrnsDestinationConfig *destinations;",
            "    size_t destination_count;",
            "    const PrnsCapability *required_capabilities;",
            "    size_t required_capability_count;",
            "    PrnsPersistenceConfig persistence;",
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
            "/* Returned host views remain valid until prns_host_release. */",
            "/* Result views remain valid until prns_command_release. */",
            "/* Event views remain valid until prns_event_release. */",
            "/* A resource may be claimed once and remains owned after its event is released. */",
            "/* out_chunk remains valid until the next call or release on this stream. */",
            "",
        ]
    )
    lines.extend(c_operation_declarations(schema))
    lines.extend(c_command_declarations(schema))
    lines.extend(
        [
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
    if isinstance(value, dict):
        return f"tuple[{python_type(value['array'])}, ...]"
    return {
        "bytes": "bytes",
        "bool": "bool",
        "string": "str",
        "i16": "int",
        "i64": "int",
        "u8": "int",
        "u16": "int",
        "u32": "int",
        "safeInt": "int",
        "safeUint": "int",
        "u64": "int",
        "u128": "int",
        "ResourceStream": "Any",
    }.get(value, value)


def python_record(record):
    lines = ["@dataclass(frozen=True, slots=True)", f"class {record['name']}:"]
    if not record["fields"]:
        lines.append("    pass")
    for field in record["fields"]:
        field_type = python_type(field["type"])
        if field.get("optional", False):
            field_type = f"{field_type} | None"
        lines.append(f"    {snake(field['name'])}: {field_type}")
    if record["name"] == "DestinationName":
        lines.extend(
            [
                "",
                "    def __post_init__(self):",
                "        if not self.app_name or not self.aspects or any(not value for value in self.aspects):",
                '            raise ValueError("a destination requires a non-empty app name and aspects")',
            ]
        )
    lines.append("")
    return lines


def python_output(schema):
    lines = [
        "from __future__ import annotations",
        "",
        "from dataclasses import dataclass",
        "from enum import IntEnum",
        "from typing import Any, Generic, Protocol, TypeAlias, TypeVar",
        "",
        f"HOST_CONTRACT_ABI = {schema['abi']}",
        f"SCHEMA_VERSION = {schema['schemaVersion']}",
        f'PRODUCT_VERSION = "{schema["productVersion"]}"',
    ]
    for item in schema["fixedBytes"]:
        lines.append(f"{screaming(item['name'])}_LENGTH = {item['length']}")
    for scalar in schema["scalars"]:
        if scalar["minimum"] != 0:
            lines.append(f"{screaming(scalar['name'])}_MIN = {scalar['minimum']}")
        lines.append(f"{screaming(scalar['name'])}_MAX = {scalar['maximum']}")
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
    for record in schema.get("records", []):
        lines.extend(python_record(record))
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
    lines.extend(python_raw_protocol(schema))
    return "\n".join(lines)


def dotnet_type(value):
    if isinstance(value, dict):
        return f"ImmutableArray<{dotnet_type(value['array'])}>"
    return {
        "bytes": "ReadOnlyMemory<byte>",
        "bool": "bool",
        "string": "string",
        "i16": "short",
        "i64": "long",
        "u8": "byte",
        "u16": "ushort",
        "u32": "uint",
        "safeInt": "long",
        "safeUint": "ulong",
        "u64": "ulong",
        "u128": "UInt128",
        "ResourceStream": "ResourceStream",
    }.get(value, value)


def dotnet_record(record):
    parameters = []
    for field in record["fields"]:
        field_type = dotnet_type(field["type"])
        if field.get("optional", False):
            field_type += "?"
        field_name = field["name"][0].upper() + field["name"][1:]
        parameters.append(f"{field_type} {field_name}")
    return f"public sealed record {record['name']}({', '.join(parameters)});"


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
    for scalar in schema["scalars"]:
        scalar_type = {"i64": "long", "u64": "ulong"}[scalar["storage"]]
        if scalar["minimum"] != 0:
            lines.append(
                f"    public const {scalar_type} {scalar['name'][0].upper() + scalar['name'][1:]}Min = {scalar['minimum']};"
            )
        lines.append(
            f"    public const {scalar_type} {scalar['name'][0].upper() + scalar['name'][1:]}Max = {scalar['maximum']};"
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
    for record in schema.get("records", []):
        lines.extend([dotnet_record(record), ""])
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
    lines.extend(dotnet_raw_protocol(schema))
    return "\n".join(lines)


def go_type(value):
    if isinstance(value, dict):
        return f"[]{go_type(value['array'])}"
    return {
        "bytes": "[]byte",
        "bool": "bool",
        "string": "string",
        "i16": "int16",
        "i64": "int64",
        "u8": "uint8",
        "u16": "uint16",
        "u32": "uint32",
        "safeInt": "int64",
        "safeUint": "uint64",
        "u64": "uint64",
        "u128": "UInt128",
    }.get(value, value)


def go_record(record):
    lines = [f"type {record['name']} struct {{"]
    for field in record["fields"]:
        field_type = go_type(field["type"])
        if field.get("optional", False):
            field_type = f"*{field_type}"
        field_name = field["name"][0].upper() + field["name"][1:]
        lines.append(f"\t{field_name} {field_type}")
    lines.extend(["}", ""])
    return lines


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
    for scalar in schema["scalars"]:
        scalar_type = {"i64": "int64", "u64": "uint64"}[scalar["storage"]]
        if scalar["minimum"] != 0:
            lines.append(
                f"const {scalar['name'][0].upper() + scalar['name'][1:]}Min {scalar_type} = {scalar['minimum']}"
            )
        lines.append(
            f"const {scalar['name'][0].upper() + scalar['name'][1:]}Max {scalar_type} = {scalar['maximum']}"
        )
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
    for record in schema.get("records", []):
        lines.extend(go_record(record))
    lines.extend(
        [
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
    lines.extend(go_raw_protocol(schema))
    return "\n".join(lines)


def swift_type(value):
    if isinstance(value, dict):
        return f"[{swift_type(value['array'])}]"
    return {
        "bytes": "[UInt8]",
        "bool": "Bool",
        "string": "String",
        "i16": "Int16",
        "i64": "Int64",
        "u8": "UInt8",
        "u16": "UInt16",
        "u32": "UInt32",
        "safeInt": "Int64",
        "safeUint": "UInt64",
        "u64": "UInt64",
        "u128": "UInt128",
        "ResourceStream": "any ResourceStream",
    }.get(value, value)


def swift_record(record):
    conformances = "Hashable, Sendable" if record.get("hashable", False) else "Sendable"
    lines = [f"public struct {record['name']}: {conformances} {{"]
    for field in record["fields"]:
        field_type = swift_type(field["type"])
        if field.get("optional", False):
            field_type += "?"
        lines.append(f"    public let {field['name']}: {field_type}")
    parameters = []
    for field in record["fields"]:
        field_type = swift_type(field["type"])
        if field.get("optional", False):
            field_type += "?"
        parameters.append(f"{field['name']}: {field_type}")
    lines.extend(["", f"    public init({', '.join(parameters)}) {{"])
    for field in record["fields"]:
        lines.append(f"        self.{field['name']} = {field['name']}")
    lines.extend(["    }", "}", ""])
    return lines


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
    for scalar in schema["scalars"]:
        scalar_type = {"i64": "Int64", "u64": "UInt64"}[scalar["storage"]]
        if scalar["minimum"] != 0:
            lines.append(
                f"    public static let {lower_first(scalar['name'])}Min: {scalar_type} = {scalar['minimum']}"
            )
        lines.append(
            f"    public static let {lower_first(scalar['name'])}Max: {scalar_type} = {scalar['maximum']}"
        )
    for key, value in schema["limits"].items():
        lines.append(f"    public static let balanced{key[0].upper() + key[1:]} = {value}")
    lines.extend(["}", ""])
    for enum in schema["enums"]:
        lines.append(f"public enum {enum['name']}: UInt32, Sendable {{")
        for value in enum["values"]:
            lines.append(f"    case {swift_identifier(value['name'])} = {value['value']}")
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
            "public protocol ResourceStream: AnyObject, AsyncSequence, Sendable",
            "where Element == [UInt8] {",
            "    var totalBytes: UInt64 { get }",
            "    func close()",
            "}",
            "",
        ]
    )
    record_index = lines.index("public protocol ResourceStream: AnyObject, AsyncSequence, Sendable")
    record_lines = []
    for record in schema.get("records", []):
        record_lines.extend(swift_record(record))
    lines[record_index:record_index] = record_lines
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
    lines.extend(swift_raw_protocol(schema))
    return "\n".join(lines)


def kotlin_type(value):
    if isinstance(value, dict):
        return f"List<{kotlin_type(value['array'])}>"
    return {
        "bytes": "Bytes",
        "bool": "Boolean",
        "string": "String",
        "i16": "Int",
        "i64": "Long",
        # Kotlin unsigned values compile to name-mangled JVM methods and
        # synthetic constructors, which makes an otherwise shared Kotlin/Java
        # SDK unusable from Java. Int and Long preserve every ABI bit while
        # producing an ordinary, stable JVM surface for both languages.
        "u8": "Int",
        "u16": "Int",
        "u32": "Long",
        "safeInt": "Long",
        "safeUint": "Long",
        "u64": "ULong",
        "u128": "BigInteger",
    }.get(value, value)


def kotlin_name(name):
    if name == "interface":
        return "`interface`"
    return lower_first(name)


def kotlin_record(record):
    lines = [f"data class {record['name']}("]
    for field in record["fields"]:
        field_type = kotlin_type(field["type"])
        if field.get("optional", False):
            field_type += "?"
        lines.append(f"    val {kotlin_name(field['name'])}: {field_type},")
    lines.extend([")", ""])
    return lines


def kotlin_output(schema):
    lines = [
        "package rs.reticulum.prns",
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
    for scalar in schema["scalars"]:
        if scalar["minimum"] != 0:
            lines.append(
                f"    const val {screaming(scalar['name'])}_MIN = {scalar['minimum']}L"
            )
        lines.append(f"    const val {screaming(scalar['name'])}_MAX = {scalar['maximum']}L")
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
            "interface ResourceStream : AutoCloseable {",
            "    val totalBytes: ULong",
            "    fun next(maximumBytes: Int): ResourceChunk",
            "}",
            "",
            "data class ResourceChunk(val bytes: Bytes, val finished: Boolean)",
            "",
        ]
    )
    record_index = lines.index("interface ResourceStream : AutoCloseable {")
    record_lines = []
    for record in schema.get("records", []):
        record_lines.extend(kotlin_record(record))
    lines[record_index:record_index] = record_lines
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
    lines.extend(kotlin_raw_protocol(schema))
    return "\n".join(lines)


def julia_type(value):
    if isinstance(value, dict):
        return f"Vector{{{julia_type(value['array'])}}}"
    return {
        "bytes": "Vector{UInt8}",
        "bool": "Bool",
        "string": "String",
        "i16": "Int16",
        "i64": "Int64",
        "u8": "UInt8",
        "u16": "UInt16",
        "u32": "UInt32",
        "safeInt": "Int64",
        "safeUint": "UInt64",
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


def julia_record(record):
    lines = [f"struct {record['name']}"]
    for field in record["fields"]:
        field_type = julia_type(field["type"])
        if field.get("optional", False):
            field_type = f"Union{{Nothing,{field_type}}}"
        lines.append(f"    {julia_name(field['name'])}::{field_type}")
    lines.extend(["end", ""])
    return lines


def julia_output(schema):
    lines = [
        f"const HOST_CONTRACT_ABI = UInt32({schema['abi']})",
        f"const HOST_SCHEMA_VERSION = UInt32({schema['schemaVersion']})",
        f'const PRODUCT_VERSION = "{schema["productVersion"]}"',
    ]
    for item in schema["fixedBytes"]:
        lines.append(f"const {screaming(item['name'])}_LENGTH = {item['length']}")
    for scalar in schema["scalars"]:
        scalar_type = {"i64": "Int64", "u64": "UInt64"}[scalar["storage"]]
        if scalar["minimum"] != 0:
            lines.append(
                f"const {screaming(scalar['name'])}_MIN = {scalar_type}({scalar['minimum']})"
            )
        lines.append(
            f"const {screaming(scalar['name'])}_MAX = {scalar_type}({scalar['maximum']})"
        )
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
    for record in schema.get("records", []):
        lines.extend(julia_record(record))
    lines.extend(["abstract type ResourceStream end", ""])
    for union in schema["unions"]:
        lines.extend([f"abstract type {union['name']} end", ""])
    for union in schema["unions"]:
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
    lines.extend(julia_raw_protocol(schema))
    return "\n".join(lines)


def vectors_output(schema):
    return (
        json.dumps(
            {
                "schemaVersion": schema["schemaVersion"],
                "abi": schema["abi"],
                "productVersion": schema["productVersion"],
                "scalars": {
                    scalar["name"]: {
                        "storage": scalar["storage"],
                        "minimum": str(scalar["minimum"]),
                        "maximum": str(scalar["maximum"]),
                    }
                    for scalar in schema["scalars"]
                },
                "integerChecks": {
                    "safeInt": {
                        "typescript": "number",
                        "accepted": ["-9007199254740991", "0", "9007199254740991"],
                        "rejected": ["-9007199254740992", "9007199254740992"],
                    },
                    "safeUint": {
                        "typescript": "number",
                        "accepted": ["0", "9007199254740991"],
                        "rejected": ["-1", "9007199254740992"],
                    },
                    "u64": {
                        "typescript": "bigint",
                        "accepted": ["0", "9007199254740992", "18446744073709551615"],
                    },
                },
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
                "records": {
                    record["name"]: record["fields"]
                    for record in schema.get("records", [])
                },
                "unions": {
                    union["name"]: {
                        case["name"]: case["value"] for case in union["cases"]
                    }
                    for union in schema["unions"]
                },
                "handles": schema["handles"],
                "commandProjection": schema["commandProjection"],
                "operations": schema["operations"],
                "contractChecks": [
                    {
                        "requiredAbi": schema["abi"],
                        "requiredSchemaVersion": schema["schemaVersion"],
                        "requiredProductVersion": schema["productVersion"],
                        "outcome": "Compatible",
                    },
                    {
                        "requiredAbi": schema["abi"] + 1,
                        "requiredSchemaVersion": schema["schemaVersion"],
                        "requiredProductVersion": schema["productVersion"],
                        "outcome": "ContractMismatch",
                    },
                    {
                        "requiredAbi": schema["abi"],
                        "requiredSchemaVersion": schema["schemaVersion"] + 1,
                        "requiredProductVersion": schema["productVersion"],
                        "outcome": "ContractMismatch",
                    },
                    {
                        "requiredAbi": schema["abi"],
                        "requiredSchemaVersion": schema["schemaVersion"],
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


def require_functions(path, names, prefix=""):
    content = path.read_text()
    missing = [
        name
        for name in names
        if re.search(rf"\b{re.escape(prefix + name)}\s*\(", content) is None
    ]
    if missing:
        joined = ", ".join(missing)
        raise ValueError(
            f"host SDK high-level surface is stale: {path.relative_to(ROOT)}: {joined}"
        )


def verify_high_level_surfaces(schema):
    union_name = schema["commandProjection"]["union"]
    command = next(item for item in schema["unions"] if item["name"] == union_name)
    cases = [case["name"] for case in command["cases"]]
    kotlin_names = [lower_first(name) for name in cases]
    swift_names = list(kotlin_names)
    go_names = [name.replace("Tcp", "TCP").replace("Udp", "UDP") for name in cases]
    julia_names = [snake(name) for name in cases]

    require_functions(KOTLIN_HOST_PATH, kotlin_names, "suspend fun ")
    kotlin_host = KOTLIN_HOST_PATH.read_text()
    missing_async = [
        name
        for name in kotlin_names
        if re.search(rf"\bfun\s+{re.escape(name)}Async\s*\(", kotlin_host) is None
    ]
    if missing_async:
        joined = ", ".join(missing_async)
        raise ValueError(f"JVM async host surface is stale: {joined}")

    require_functions(SWIFT_HOST_PATH, swift_names, "public func ")
    require_functions(GO_HOST_PATH, go_names, "Host) ")
    require_functions(JULIA_COMMAND_PATH, julia_names)
    julia_module = JULIA_MODULE_PATH.read_text()
    missing_exports = [
        name
        for name in julia_names
        if re.search(rf"^export\s+{re.escape(name)}$", julia_module, re.MULTILINE)
        is None
    ]
    if missing_exports:
        joined = ", ".join(missing_exports)
        raise ValueError(f"Julia high-level exports are stale: {joined}")

    jvm_sources = "\n".join(
        path.read_text()
        for path in (KOTLIN_HOST_PATH, KOTLIN_EVENTS_PATH, KOTLIN_UPLOAD_PATH)
    )
    for required in (
        "executeAsync(",
        "nextAsync(",
        "writeAsync(",
        "finishAsync(",
    ):
        if required not in jvm_sources:
            raise ValueError(f"JVM async bridge is stale: missing {required}")
    blocking_sources = "\n".join(
        path.read_text() for path in (KOTLIN_COMMAND_PATH, KOTLIN_EVENTS_PATH)
    )
    for forbidden in ("awaitBlocking(", "nextBlocking("):
        if forbidden in blocking_sources:
            raise ValueError(f"JVM blocking bridge remains public: {forbidden}")


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
    if args.check:
        verify_high_level_surfaces(schema)


if __name__ == "__main__":
    main()
