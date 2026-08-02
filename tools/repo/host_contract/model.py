from dataclasses import dataclass
import re


TYPE_NAME = re.compile(r"^[A-Z][A-Za-z0-9]*$")
FIELD_NAME = re.compile(r"^[a-z][A-Za-z0-9]*$")
SCALAR_NAME = re.compile(r"^[a-z][A-Za-z0-9]*$")
PRODUCT_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
PRIMITIVES = frozenset(
    {"bytes", "bool", "string", "i16", "i64", "u8", "u16", "u32", "u64", "u128"}
)
OPERATION_TYPES = frozenset(
    {
        "ContractInfo",
        "HostOptions",
        "Lifecycle",
        "CommandResult",
        "ResourceChunk",
        "ReadinessCallback",
        "opaquePointer",
        "size",
    }
)
OPERATION_KINDS = frozenset(
    {
        "blocking",
        "claim",
        "create",
        "finish",
        "interrupt",
        "pull",
        "query",
        "readiness",
        "release",
        "stop",
        "write",
    }
)
INTEGER_BOUNDS = {
    "i16": (-(2**15), 2**15 - 1),
    "i64": (-(2**63), 2**63 - 1),
    "u8": (0, 2**8 - 1),
    "u16": (0, 2**16 - 1),
    "u32": (0, 2**32 - 1),
    "u64": (0, 2**64 - 1),
    "u128": (0, 2**128 - 1),
}


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def ensure_shape(value, required, optional, owner):
    if not isinstance(value, dict):
        raise ValueError(f"invalid {owner} shape")
    keys = set(value)
    missing = required - keys
    unknown = keys - required - optional
    if missing:
        raise ValueError(f"missing {owner} keys: {sorted(missing)}")
    if unknown:
        raise ValueError(f"unknown {owner} keys: {sorted(unknown)}")


def ensure_name(name, pattern, owner):
    if not isinstance(name, str) or pattern.fullmatch(name) is None:
        raise ValueError(f"invalid {owner} name {name!r}")


def validate_type(value, known_types, owner):
    if isinstance(value, str):
        if value not in known_types:
            raise ValueError(f"unknown type {value} in {owner}")
        return
    ensure_shape(value, {"array"}, set(), f"type in {owner}")
    validate_type(value["array"], known_types, owner)


def validate_fields(fields, known_types, owner):
    if not isinstance(fields, list):
        raise ValueError(f"invalid fields in {owner}")
    names = set()
    for field in fields:
        ensure_shape(field, {"name", "type"}, {"optional"}, f"field in {owner}")
        name = field["name"]
        ensure_name(name, FIELD_NAME, f"field in {owner}")
        if name in names:
            raise ValueError(f"duplicate field {name} in {owner}")
        validate_type(field["type"], known_types, owner)
        if "optional" in field and field["optional"] is not True:
            raise ValueError(f"invalid optional marker in {owner}.{name}")
        names.add(name)


@dataclass(frozen=True)
class Scalar:
    name: str
    storage: str
    minimum: int
    maximum: int


@dataclass(frozen=True)
class Handle:
    name: str
    release: str
    thread_safe: bool


@dataclass(frozen=True)
class Parameter:
    name: str
    type_name: str
    passing: str


@dataclass(frozen=True)
class Receiver:
    type_name: str
    mutable: bool
    consumed: bool


@dataclass(frozen=True)
class Result:
    type_name: str
    ownership: str
    valid_until: str | None
    exclusive: bool


@dataclass(frozen=True)
class Operation:
    name: str
    kind: str
    receiver: Receiver | None
    parameters: tuple[Parameter, ...]
    result: Result | None
    interrupt: str | None
    readiness: str | None
    status: bool


@dataclass(frozen=True)
class ContractModel:
    schema_version: int
    abi: int
    product_version: str
    scalars: tuple[Scalar, ...]
    handles: tuple[Handle, ...]
    operations: tuple[Operation, ...]

    @classmethod
    def parse(cls, schema):
        ensure_shape(
            schema,
            {
                "schemaVersion",
                "abi",
                "productVersion",
                "scalars",
                "fixedBytes",
                "handles",
                "records",
                "limits",
                "enums",
                "unions",
                "commandProjection",
                "operations",
            },
            set(),
            "host contract",
        )
        if schema["schemaVersion"] != 1:
            raise ValueError("unsupported host contract schema version")
        if not isinstance(schema["abi"], int) or schema["abi"] != 1:
            raise ValueError("unsupported host contract ABI")
        if (
            not isinstance(schema["productVersion"], str)
            or PRODUCT_VERSION.fullmatch(schema["productVersion"]) is None
        ):
            raise ValueError("invalid product version")
        scalars = parse_scalars(schema["scalars"])
        handles = parse_handles(schema["handles"])
        validate_limits(schema["limits"])
        validate_vocabulary(schema, scalars, handles)
        operations = parse_operations(schema, handles)
        return cls(
            schema_version=schema["schemaVersion"],
            abi=schema["abi"],
            product_version=schema["productVersion"],
            scalars=tuple(scalars),
            handles=tuple(handles),
            operations=tuple(operations),
        )


def parse_scalars(values):
    if not isinstance(values, list):
        raise ValueError("invalid scalars")
    result = []
    names = set()
    for value in values:
        ensure_shape(
            value,
            {"name", "storage", "minimum", "maximum"},
            set(),
            "scalar",
        )
        name = value["name"]
        ensure_name(name, SCALAR_NAME, "scalar")
        if name in names or name in PRIMITIVES:
            raise ValueError(f"duplicate contract type {name}")
        storage = value["storage"]
        if storage not in PRIMITIVES - {"bytes", "bool", "string"}:
            raise ValueError(f"invalid scalar storage {storage}")
        minimum = value["minimum"]
        maximum = value["maximum"]
        if not isinstance(minimum, int) or not isinstance(maximum, int) or minimum > maximum:
            raise ValueError(f"invalid scalar bounds {name}")
        storage_minimum, storage_maximum = INTEGER_BOUNDS[storage]
        if minimum < storage_minimum or maximum > storage_maximum:
            raise ValueError(f"scalar bounds exceed storage {name}")
        if name == "safeUint" and (minimum != 0 or maximum != 9_007_199_254_740_991):
            raise ValueError("safeUint must match the interoperable safe integer range")
        if name == "safeInt" and (
            minimum != -9_007_199_254_740_991
            or maximum != 9_007_199_254_740_991
        ):
            raise ValueError("safeInt must match the interoperable safe integer range")
        names.add(name)
        result.append(Scalar(name, storage, minimum, maximum))
    return result


def validate_limits(value):
    expected = {
        "pendingCommands",
        "applicationEvents",
        "retainedEventBytes",
        "diagnostics",
    }
    ensure_shape(value, expected, set(), "limits")
    for name, limit in value.items():
        if not isinstance(limit, int) or isinstance(limit, bool) or limit < 1:
            raise ValueError(f"invalid limit {name}")


def parse_handles(values):
    if not isinstance(values, list):
        raise ValueError("invalid handles")
    result = []
    names = set()
    for value in values:
        ensure_shape(value, {"name", "release", "threadSafe"}, set(), "handle")
        name = value["name"]
        ensure_name(name, TYPE_NAME, "handle")
        ensure_name(value["release"], FIELD_NAME, "handle release")
        if name in names:
            raise ValueError(f"duplicate contract type {name}")
        if not isinstance(value["threadSafe"], bool):
            raise ValueError(f"invalid thread safety for {name}")
        names.add(name)
        result.append(Handle(name, value["release"], value["threadSafe"]))
    return result


def validate_vocabulary(schema, scalars, handles):
    named_types = {value.name for value in scalars} | {value.name for value in handles}
    for collection in ("fixedBytes", "enums", "records", "unions"):
        values = schema[collection]
        if not isinstance(values, list):
            raise ValueError(f"invalid {collection}")
        for item in values:
            required = {
                "fixedBytes": {"name", "length"},
                "enums": {"name", "values"},
                "records": {"name", "fields"},
                "unions": {"name", "cases"},
            }[collection]
            optional = {
                "fixedBytes": {"secret"},
                "enums": set(),
                "records": {"hashable"},
                "unions": set(),
            }[collection]
            ensure_shape(item, required, optional, collection)
            name = item["name"]
            ensure_name(name, TYPE_NAME, collection)
            if name in named_types:
                raise ValueError(f"duplicate contract type {name}")
            named_types.add(name)
    known_types = named_types | PRIMITIVES
    for item in schema["fixedBytes"]:
        if set(item) - {"name", "length", "secret"}:
            raise ValueError(f"invalid fixed byte shape {item['name']}")
        if not isinstance(item.get("length"), int) or item["length"] < 1:
            raise ValueError(f"invalid fixed byte type {item['name']}")
        if "secret" in item and item["secret"] is not True:
            raise ValueError(f"invalid secret marker {item['name']}")
    for enum in schema["enums"]:
        if set(enum) != {"name", "values"} or not isinstance(enum["values"], list):
            raise ValueError(f"invalid enum shape {enum['name']}")
        names = set()
        discriminants = set()
        for value in enum["values"]:
            ensure_shape(value, {"name", "value"}, set(), f"value in {enum['name']}")
            ensure_name(value["name"], TYPE_NAME, f"value in {enum['name']}")
            discriminant = value["value"]
            if not isinstance(discriminant, int) or not 0 <= discriminant <= 0xFFFF_FFFF:
                raise ValueError(f"invalid value in {enum['name']}")
            if value["name"] in names or discriminant in discriminants:
                raise ValueError(f"duplicate value in {enum['name']}")
            names.add(value["name"])
            discriminants.add(discriminant)
    for record in schema["records"]:
        if set(record) - {"name", "fields", "hashable"}:
            raise ValueError(f"invalid record shape {record['name']}")
        if "hashable" in record and record["hashable"] is not True:
            raise ValueError(f"invalid hashable marker {record['name']}")
        validate_fields(record["fields"], known_types, record["name"])
    for union in schema["unions"]:
        if set(union) != {"name", "cases"} or not isinstance(union["cases"], list):
            raise ValueError(f"invalid union shape {union['name']}")
        names = set()
        discriminants = set()
        for case in union["cases"]:
            ensure_shape(case, {"name", "value", "fields"}, set(), f"case in {union['name']}")
            ensure_name(case["name"], TYPE_NAME, f"case in {union['name']}")
            discriminant = case["value"]
            if not isinstance(discriminant, int) or not 0 <= discriminant <= 0xFFFF_FFFF:
                raise ValueError(f"invalid case in {union['name']}")
            if case["name"] in names or discriminant in discriminants:
                raise ValueError(f"duplicate case in {union['name']}")
            validate_fields(case["fields"], known_types, f"{union['name']}.{case['name']}")
            names.add(case["name"])
            discriminants.add(discriminant)
    validate_projected_names(named_types)
    validate_value_cycles(schema, named_types)
    validate_mirrored_unions(schema)
    validate_flattened_unions(schema)


def validate_projected_names(names):
    for transform in (str.lower, snake):
        projected = {}
        for name in names:
            target = transform(name)
            if target in projected and projected[target] != name:
                raise ValueError(f"projected name collision: {projected[target]} and {name}")
            projected[target] = name


def referenced_names(value):
    if isinstance(value, dict):
        return {value["array"]}
    return {value}


def validate_value_cycles(schema, named_types):
    value_names = {item["name"] for item in schema["records"] + schema["unions"]}
    graph = {name: set() for name in value_names}
    for record in schema["records"]:
        for field in record["fields"]:
            graph[record["name"]].update(referenced_names(field["type"]) & value_names)
    for union in schema["unions"]:
        for case in union["cases"]:
            for field in case["fields"]:
                graph[union["name"]].update(referenced_names(field["type"]) & value_names)
    visiting = set()
    complete = set()

    def visit(name):
        if name in complete:
            return
        if name in visiting:
            raise ValueError(f"recursive value type {name}")
        visiting.add(name)
        for dependency in graph[name]:
            visit(dependency)
        visiting.remove(name)
        complete.add(name)

    for name in graph:
        visit(name)


def validate_mirrored_unions(schema):
    for union_name, enum_name in (
        ("CommandOutcome", "CommandOutcomeKind"),
        ("CommandFailure", "CommandFailureKind"),
        ("ApplicationEvent", "ApplicationEventKind"),
        ("DiagnosticEvent", "DiagnosticEventKind"),
    ):
        union = next(item for item in schema["unions"] if item["name"] == union_name)
        enum = next(item for item in schema["enums"] if item["name"] == enum_name)
        union_cases = {item["name"]: item["value"] for item in union["cases"]}
        enum_values = {item["name"]: item["value"] for item in enum["values"]}
        if union_cases != enum_values:
            raise ValueError(f"{union_name} and {enum_name} disagree")


def validate_flattened_unions(schema):
    for union_name in ("InterfaceConfig", "DestinationConfig"):
        union = next(item for item in schema["unions"] if item["name"] == union_name)
        fields = {}
        for case in union["cases"]:
            for field in case["fields"]:
                prior = fields.get(field["name"])
                if prior is not None and prior != field["type"]:
                    raise ValueError(f"C flattening collision in {union_name}.{field['name']}")
                fields[field["name"]] = field["type"]


def parse_operations(schema, handles):
    handle_by_name = {handle.name: handle for handle in handles}
    vocabulary = {
        item["name"]
        for collection in ("fixedBytes", "records", "enums", "unions")
        for item in schema[collection]
    }
    known_types = vocabulary | set(handle_by_name) | PRIMITIVES | OPERATION_TYPES | {
        value["name"] for value in schema["scalars"]
    }
    values = schema["operations"]
    if not isinstance(values, list):
        raise ValueError("invalid operations")
    operations = []
    names = set()
    symbols = set()
    for value in values:
        ensure_shape(
            value,
            {"name", "kind", "parameters", "status"},
            {"receiver", "result", "interrupt", "readiness"},
            "operation",
        )
        name = value["name"]
        ensure_name(name, FIELD_NAME, "operation")
        kind = value["kind"]
        if kind not in OPERATION_KINDS:
            raise ValueError(f"invalid operation kind in {name}")
        symbol = f"prns_{snake(name)}"
        if name in names or symbol in symbols:
            raise ValueError(f"duplicate operation {name}")
        receiver = parse_receiver(value.get("receiver"), handle_by_name, name)
        parameters = parse_parameters(value["parameters"], known_types, name)
        result = parse_result(value.get("result"), known_types, name)
        for relation in ("interrupt", "readiness"):
            target = value.get(relation)
            if target is not None:
                ensure_name(target, FIELD_NAME, f"{relation} in {name}")
        if not isinstance(value["status"], bool):
            raise ValueError(f"invalid status marker in {name}")
        operations.append(
            Operation(
                name=name,
                kind=kind,
                receiver=receiver,
                parameters=tuple(parameters),
                result=result,
                interrupt=value.get("interrupt"),
                readiness=value.get("readiness"),
                status=value["status"],
            )
        )
        names.add(name)
        symbols.add(symbol)
    operation_by_name = {operation.name: operation for operation in operations}
    for operation in operations:
        if operation.kind == "release":
            if (
                operation.receiver is None
                or not operation.receiver.consumed
                or operation.parameters
                or operation.result is not None
                or operation.status
            ):
                raise ValueError(f"invalid release operation {operation.name}")
        elif operation.receiver is not None and operation.receiver.consumed:
            raise ValueError(f"non-release operation consumes receiver {operation.name}")
        if operation.interrupt is not None:
            validate_relation(operation_by_name, operation, operation.interrupt, "interrupt")
        if operation.readiness is not None:
            validate_relation(operation_by_name, operation, operation.readiness, "readiness")
        if operation.result is not None and operation.result.valid_until is not None:
            if operation.result.valid_until == "resourceStreamNextOrRelease":
                continue
            if operation.result.valid_until not in operation_by_name:
                raise ValueError(f"unknown lifetime operation {operation.result.valid_until}")
    for handle in handles:
        release = operation_by_name.get(handle.release)
        if release is None or release.kind != "release":
            raise ValueError(f"missing release operation for {handle.name}")
        if release.receiver is None or release.receiver.type_name != handle.name:
            raise ValueError(f"invalid release operation for {handle.name}")
    parse_command_projection(schema, handle_by_name, names, symbols)
    return operations


def parse_receiver(value, handles, owner):
    if value is None:
        return None
    ensure_shape(value, {"type", "mutable"}, {"consumed"}, f"receiver in {owner}")
    if value["type"] not in handles:
        raise ValueError(f"unknown receiver handle {value['type']} in {owner}")
    if not isinstance(value["mutable"], bool):
        raise ValueError(f"invalid receiver mutability in {owner}")
    consumed = value.get("consumed", False)
    if not isinstance(consumed, bool):
        raise ValueError(f"invalid receiver consumption in {owner}")
    return Receiver(value["type"], value["mutable"], consumed)


def parse_parameters(values, known_types, owner):
    if not isinstance(values, list):
        raise ValueError(f"invalid parameters in {owner}")
    result = []
    names = set()
    for value in values:
        ensure_shape(value, {"name", "type", "passing"}, set(), f"parameter in {owner}")
        name = value["name"]
        ensure_name(name, FIELD_NAME, f"parameter in {owner}")
        if name in names:
            raise ValueError(f"duplicate parameter {name} in {owner}")
        if value["type"] not in known_types:
            raise ValueError(f"unknown parameter type {value['type']} in {owner}")
        if value["passing"] not in {"value", "borrow", "optionalBorrow"}:
            raise ValueError(f"invalid parameter passing in {owner}.{name}")
        result.append(Parameter(name, value["type"], value["passing"]))
        names.add(name)
    return result


def parse_result(value, known_types, owner):
    if value is None:
        return None
    ensure_shape(value, {"type", "ownership"}, {"validUntil", "exclusive"}, f"result in {owner}")
    if value["type"] not in known_types:
        raise ValueError(f"unknown result type {value['type']} in {owner}")
    ownership = value["ownership"]
    if ownership not in {"owned", "borrowed", "copied"}:
        raise ValueError(f"invalid result ownership in {owner}")
    valid_until = value.get("validUntil")
    if ownership == "borrowed" and valid_until is None:
        raise ValueError(f"borrowed result lacks lifetime in {owner}")
    if ownership != "borrowed" and valid_until is not None:
        raise ValueError(f"non-borrowed result has lifetime in {owner}")
    exclusive = value.get("exclusive", False)
    if not isinstance(exclusive, bool) or exclusive and ownership != "owned":
        raise ValueError(f"invalid exclusive result in {owner}")
    return Result(value["type"], ownership, valid_until, exclusive)


def validate_relation(operations, owner, target_name, kind):
    target = operations.get(target_name)
    if target is None or target.kind != kind:
        raise ValueError(f"invalid {kind} relation in {owner.name}")
    if owner.receiver is None or target.receiver is None:
        raise ValueError(f"receiverless {kind} relation in {owner.name}")
    if owner.receiver.type_name != target.receiver.type_name:
        raise ValueError(f"mismatched {kind} receiver in {owner.name}")


def parse_command_projection(schema, handles, operation_names, operation_symbols):
    value = schema["commandProjection"]
    ensure_shape(value, {"union", "receiver", "result", "cPrefix"}, set(), "command projection")
    unions = {item["name"] for item in schema["unions"]}
    if value["union"] not in unions:
        raise ValueError("unknown command union")
    if value["receiver"] not in handles or value["result"] not in handles:
        raise ValueError("unknown command projection handle")
    if value["cPrefix"] != "prns_host_":
        raise ValueError("invalid command C prefix")
    union = next(item for item in schema["unions"] if item["name"] == value["union"])
    for case in union["cases"]:
        operation_name = f"host{case['name']}"
        symbol = f"{value['cPrefix']}{snake(case['name'])}"
        if operation_name in operation_names or symbol in operation_symbols:
            raise ValueError(f"projected command collision {case['name']}")


def validate_contract(schema):
    return ContractModel.parse(schema)
