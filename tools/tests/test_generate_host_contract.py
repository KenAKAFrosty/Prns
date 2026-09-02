import copy
import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
GENERATOR_PATH = ROOT / "tools" / "repo" / "generate-host-contract.py"
SPEC = importlib.util.spec_from_file_location("generate_host_contract", GENERATOR_PATH)
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


def canonical_schema():
    return json.loads(GENERATOR.SCHEMA_PATH.read_text())


class GenerateHostContractTests(unittest.TestCase):
    def test_canonical_schema_is_valid(self):
        GENERATOR.validate(canonical_schema())

    def test_handwritten_command_failure_decoders_cover_the_contract(self):
        missing = GENERATOR.missing_command_failure_decoder_cases(canonical_schema())
        self.assertTrue(missing)
        self.assertTrue(all(not names for names in missing.values()))

    def test_decoder_parity_reports_a_new_contract_case(self):
        schema = canonical_schema()
        failure_kind = next(
            item
            for item in schema["enums"]
            if item["name"] == "CommandFailureKind"
        )
        failure_kind["values"].append(
            {"name": "FutureFailure", "value": 10_000}
        )

        missing = GENERATOR.missing_command_failure_decoder_cases(schema)
        self.assertTrue(
            all("FutureFailure" in names for names in missing.values())
        )

    def test_unknown_schema_revision_is_rejected(self):
        schema = canonical_schema()
        schema["schemaVersion"] = 3
        with self.assertRaisesRegex(ValueError, "unsupported host contract schema version"):
            GENERATOR.validate(schema)

    def test_unknown_top_level_grammar_is_rejected(self):
        schema = canonical_schema()
        schema["compatibility"] = {}
        with self.assertRaisesRegex(ValueError, "unknown host contract keys"):
            GENERATOR.validate(schema)

    def test_foreign_callbacks_are_not_general_contract_types(self):
        schema = canonical_schema()
        schema["callbacks"] = [
            {
                "name": "DescriptorOpener",
                "parameters": [{"name": "context", "type": "opaquePointer"}],
                "result": "i64",
            }
        ]
        with self.assertRaisesRegex(ValueError, "unknown host contract keys"):
            GENERATOR.validate(schema)

    def test_supplied_pipe_contract_is_owned_and_pull_based(self):
        schema = canonical_schema()
        handles = {handle["name"] for handle in schema["handles"]}
        operations = {operation["name"]: operation for operation in schema["operations"]}
        self.assertTrue({"SuppliedPipe", "SuppliedPipeOpenRequest"} <= handles)
        self.assertEqual(
            operations["hostAttachSuppliedPipe"]["result"],
            {"type": "SuppliedPipe", "ownership": "owned"},
        )
        self.assertEqual(
            operations["suppliedPipeNextOpenRequest"]["result"],
            {"type": "SuppliedPipeOpenRequest", "ownership": "owned"},
        )
        parameter_types = {
            parameter["type"]
            for operation in operations.values()
            for parameter in operation["parameters"]
        }
        self.assertNotIn("SuppliedPipeOpenCallback", parameter_types)
        self.assertNotIn("SuppliedPipeReleaseCallback", parameter_types)

    def test_scalar_ranges_and_limits_are_rejected_before_rendering(self):
        schema = canonical_schema()
        schema["scalars"][0]["maximum"] = 2**64
        with self.assertRaisesRegex(ValueError, "scalar bounds exceed storage"):
            GENERATOR.validate(schema)

        schema = canonical_schema()
        schema["scalars"][0]["minimum"] += 1
        with self.assertRaisesRegex(ValueError, "safeInt must match"):
            GENERATOR.validate(schema)

        schema = canonical_schema()
        schema["limits"]["unbounded"] = 1
        with self.assertRaisesRegex(ValueError, "unknown limits keys"):
            GENERATOR.validate(schema)

    def test_duplicate_contract_type_names_are_rejected(self):
        schema = canonical_schema()
        schema["records"].append(
            {"name": schema["enums"][0]["name"], "fields": []}
        )
        with self.assertRaisesRegex(ValueError, "duplicate contract type"):
            GENERATOR.validate(schema)

    def test_duplicate_enum_names_and_discriminants_are_rejected(self):
        for key in ("name", "value"):
            with self.subTest(key=key):
                schema = canonical_schema()
                duplicate = copy.deepcopy(schema["enums"][0]["values"][0])
                if key == "name":
                    duplicate["value"] += 10_000
                else:
                    duplicate["name"] += "Duplicate"
                schema["enums"][0]["values"].append(duplicate)
                with self.assertRaisesRegex(ValueError, "duplicate value"):
                    GENERATOR.validate(schema)

    def test_duplicate_union_names_and_discriminants_are_rejected(self):
        for key in ("name", "value"):
            with self.subTest(key=key):
                schema = canonical_schema()
                duplicate = copy.deepcopy(schema["unions"][0]["cases"][0])
                if key == "name":
                    duplicate["value"] += 10_000
                else:
                    duplicate["name"] += "Duplicate"
                schema["unions"][0]["cases"].append(duplicate)
                with self.assertRaisesRegex(ValueError, "duplicate case"):
                    GENERATOR.validate(schema)

    def test_operation_ownership_and_command_projection_are_strict(self):
        schema = canonical_schema()
        release = next(
            operation
            for operation in schema["operations"]
            if operation["name"] == "resourceStreamRelease"
        )
        release["status"] = True
        with self.assertRaisesRegex(ValueError, "invalid release operation"):
            GENERATOR.validate(schema)

        schema = canonical_schema()
        schema["operations"][0]["kind"] = "compatibility"
        with self.assertRaisesRegex(ValueError, "invalid operation kind"):
            GENERATOR.validate(schema)

        schema = canonical_schema()
        stop = next(
            operation
            for operation in schema["operations"]
            if operation["name"] == "hostStop"
        )
        stop["name"] = "hostAnnounce"
        with self.assertRaisesRegex(ValueError, "projected command collision"):
            GENERATOR.validate(schema)

    def test_operations_generate_functions_and_raw_protocols(self):
        schema = canonical_schema()
        command = next(
            union
            for union in schema["unions"]
            if union["name"] == schema["commandProjection"]["union"]
        )
        expected_count = len(schema["operations"]) + len(command["cases"])
        self.assertEqual(len(GENERATOR.raw_operations(schema)), expected_count)
        projections = {
            "typescript": GENERATOR.ts_output(schema),
            "python": GENERATOR.python_output(schema),
            "dotnet": GENERATOR.dotnet_output(schema),
            "go": GENERATOR.go_output(schema),
            "swift": GENERATOR.swift_output(schema),
            "kotlin": GENERATOR.kotlin_output(schema),
            "julia": GENERATOR.julia_output(schema),
        }
        expected_fragments = {
            "typescript": ("interface RawHostProtocol", "readonly hostAnnounce: (host: RawHost, destination: DestinationHash, interfaceId: InterfaceId | undefined) => RawCallResult<RawOwned<RawIssuedCommand>>;"),
            "python": ("class _RawHostProtocol(Protocol):", "def host_announce(self, host: _RawHost, destination: DestinationHash, interface: InterfaceId | None) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]:"),
            "dotnet": ("internal interface IRawHostProtocol", "HostAnnounce("),
            "go": ("type rawHostProtocol interface", "hostAnnounce("),
            "swift": ("protocol RawHostProtocol", "func hostAnnounce("),
            "kotlin": ("internal interface RawHostProtocol", "fun hostAnnounce("),
            "julia": ("abstract type RawHostProtocol end", "function host_announce(protocol::RawHostProtocol"),
        }
        for language, fragments in expected_fragments.items():
            with self.subTest(language=language):
                for fragment in fragments:
                    self.assertIn(fragment, projections[language])
        c_projection = GENERATOR.c_output(schema)
        self.assertIn("PrnsStatus prns_host_announce(", c_projection)
        self.assertNotIn("prns_host_submit", c_projection)

    def test_rust_enums_own_semantic_names_discriminants_and_names(self):
        projection = GENERATOR.rust_output(canonical_schema())
        self.assertIn("pub enum BackendKind {", projection)
        self.assertIn("Native = 1,", projection)
        self.assertIn('Self::Native => "Native",', projection)
        self.assertIn("impl TryFrom<u32> for BackendKind", projection)
        self.assertIn("_ => Err(()),", projection)
        self.assertNotIn("pub enum AbiBackendKind", projection)

    def test_typescript_enums_generate_inventories_and_guards(self):
        projection = GENERATOR.ts_output(canonical_schema())
        self.assertIn("export const BACKEND_KIND_VALUES", projection)
        self.assertIn("export function isBackendKind", projection)
        self.assertIn("export const CAPABILITY_NAME_VALUES", projection)
        self.assertIn("export function isInterfaceHealth", projection)

    def test_core_has_no_parallel_semantic_enum_declarations(self):
        generated = GENERATOR.RUST_PATH.resolve()
        declarations = []
        for path in (ROOT / "prns-host" / "core" / "src").glob("*.rs"):
            if path.resolve() == generated:
                continue
            content = path.read_text()
            for enum in canonical_schema()["enums"]:
                if f"pub enum {enum['name']} " in content:
                    declarations.append((path.name, enum["name"]))
        self.assertEqual(declarations, [])

    def test_raw_protocols_preserve_types_status_and_ownership(self):
        schema = canonical_schema()
        projections = {
            "typescript": GENERATOR.ts_output(schema),
            "python": GENERATOR.python_output(schema),
            "dotnet": GENERATOR.dotnet_output(schema),
            "go": GENERATOR.go_output(schema),
            "swift": GENERATOR.swift_output(schema),
            "kotlin": GENERATOR.kotlin_output(schema),
            "julia": GENERATOR.julia_output(schema),
        }
        typed_fragments = {
            "typescript": (
                'Tag<"Succeeded", Value>',
                'Tag<"Failed", RawStatus>',
                "RawCallResult<RawOwned<RawHost>>",
                "RawCallResult<RawBorrowed<RawCommandResult>>",
                "declaredLength: bigint",
            ),
            "python": (
                "_RawCallResult[_RawOwned[_RawHost]]",
                "_RawCallResult[_RawBorrowed[_RawCommandResult]]",
                "declared_length: int",
            ),
            "dotnet": (
                "RawCallResult<RawOwned<IRawHost>>",
                "RawCallResult<RawBorrowed<IRawCommandResult>>",
                "ulong declaredLength",
            ),
            "go": (
                "rawCallResult[rawOwned[rawHost]]",
                "rawCallResult[rawBorrowed[rawCommandResult]]",
                "declaredLength uint64",
            ),
            "swift": (
                "RawCallResult<RawOwned<RawHost>>",
                "RawCallResult<RawBorrowed<RawCommandResult>>",
                "declaredLength: UInt64",
            ),
            "kotlin": (
                "RawCallResult<RawOwned<RawHost>>",
                "RawCallResult<RawBorrowed<RawCommandResult>>",
                "declaredLength: ULong",
            ),
            "julia": (
                "RawCallResult{RawOwned{RawHost}}",
                "RawCallResult{RawBorrowed{RawCommandResult}}",
                "declared_length::UInt64",
            ),
        }
        forbidden = {
            "typescript": ("argument0: unknown", ") => unknown"),
            "python": ("argument0: Any", ") -> Any"),
            "dotnet": ("object? argument0", ") object?"),
            "go": ("argument0 any", ") any"),
            "swift": ("argument0: Any", ") -> Any"),
            "kotlin": ("argument0: Any?", "): Any?"),
        }
        for language, fragments in typed_fragments.items():
            with self.subTest(language=language):
                for fragment in fragments:
                    self.assertIn(fragment, projections[language])
                for fragment in forbidden.get(language, ()):
                    self.assertNotIn(fragment, projections[language])

    def test_javascript_integer_policy_is_explicit(self):
        schema = canonical_schema()
        projection = GENERATOR.ts_output(schema)
        self.assertIn("export const SAFE_INT_MIN = -9007199254740991;", projection)
        self.assertIn("export const SAFE_INT_MAX = 9007199254740991;", projection)
        self.assertIn("export const SAFE_UINT_MAX = 9007199254740991;", projection)
        self.assertIn("readonly revision: bigint;", projection)
        self.assertIn("readonly uptimeMillis: number;", projection)
        vectors = json.loads(GENERATOR.vectors_output(schema))
        self.assertEqual(vectors["integerChecks"]["safeInt"]["typescript"], "number")
        self.assertIn("-9007199254740991", vectors["integerChecks"]["safeInt"]["accepted"])
        self.assertEqual(vectors["integerChecks"]["safeUint"]["typescript"], "number")
        self.assertEqual(vectors["integerChecks"]["u64"]["typescript"], "bigint")
        self.assertIn("18446744073709551615", vectors["integerChecks"]["u64"]["accepted"])
        self.assertIn("SAFE_UINT_MAX = 9007199254740991", GENERATOR.python_output(schema))
        self.assertIn("SafeUintMax = 9007199254740991", GENERATOR.dotnet_output(schema))
        self.assertIn("SafeUintMax uint64 = 9007199254740991", GENERATOR.go_output(schema))
        self.assertIn("safeUintMax: UInt64 = 9007199254740991", GENERATOR.swift_output(schema))
        self.assertIn("SAFE_UINT_MAX = 9007199254740991L", GENERATOR.kotlin_output(schema))
        self.assertIn("SAFE_UINT_MAX = UInt64(9007199254740991)", GENERATOR.julia_output(schema))

    def test_request_ceiling_contract_projects_across_every_language(self):
        schema = canonical_schema()
        projections = {
            "typescript": GENERATOR.ts_output(schema),
            "c": GENERATOR.c_output(schema),
            "python": GENERATOR.python_output(schema),
            "dotnet": GENERATOR.dotnet_output(schema),
            "go": GENERATOR.go_output(schema),
            "swift": GENERATOR.swift_output(schema),
            "kotlin": GENERATOR.kotlin_output(schema),
            "julia": GENERATOR.julia_output(schema),
        }
        expected = {
            "typescript": (
                "readonly maximumRequestBytes?: number;",
                "readonly maximumResponseBytes?: number;",
                'Tag<"ResponseTooLarge">',
            ),
            "c": (
                "uint8_t has_maximum_request_bytes;",
                "uint64_t maximum_request_bytes;",
                "const uint64_t *maximum_response_bytes",
                "PRNS_COMMAND_FAILURE_KIND_RESPONSE_TOO_LARGE UINT32_C(41)",
            ),
            "python": (
                "maximum_request_bytes: int | None",
                "maximum_response_bytes: int | None",
                "class CommandFailureResponseTooLarge:",
            ),
            "dotnet": (
                "ulong? MaximumRequestBytes",
                "ulong? MaximumResponseBytes",
                "public sealed record ResponseTooLarge() : CommandFailure;",
            ),
            "go": (
                "MaximumRequestBytes *uint64",
                "MaximumResponseBytes *uint64",
                "type CommandFailureResponseTooLarge struct{}",
            ),
            "swift": (
                "maximumRequestBytes: UInt64?",
                "maximumResponseBytes: UInt64?",
                "case responseTooLarge",
            ),
            "kotlin": (
                "val maximumRequestBytes: Long?",
                "val maximumResponseBytes: Long?",
                "data object CommandFailureResponseTooLarge",
            ),
            "julia": (
                "maximum_request_bytes::Union{Nothing,UInt64}",
                "maximum_response_bytes::Union{Nothing,UInt64}",
                "struct CommandFailureResponseTooLarge",
            ),
        }
        for language, fragments in expected.items():
            with self.subTest(language=language):
                for fragment in fragments:
                    self.assertIn(fragment, projections[language])

    def test_interface_routing_contract_projects_across_every_language(self):
        schema = canonical_schema()
        projections = {
            "typescript": GENERATOR.ts_output(schema),
            "c": GENERATOR.c_output(schema),
            "python": GENERATOR.python_output(schema),
            "dotnet": GENERATOR.dotnet_output(schema),
            "go": GENERATOR.go_output(schema),
            "swift": GENERATOR.swift_output(schema),
            "kotlin": GENERATOR.kotlin_output(schema),
            "julia": GENERATOR.julia_output(schema),
        }
        expected = {
            "typescript": (
                'export type InterfaceMode =',
                '  | "Internal";',
                "readonly gravity?: number;",
                "readonly routing?: InterfaceRoutingPolicy;",
            ),
            "c": (
                "PRNS_INTERFACE_MODE_INTERNAL UINT32_C(7)",
                "typedef struct PrnsInterfaceRoutingPolicy",
                "uint8_t has_announces_to_internal;",
                "const PrnsInterfaceRoutingPolicy *routing",
            ),
            "python": (
                "class InterfaceMode(IntEnum):",
                "gravity: int | None",
                "routing: InterfaceRoutingPolicy | None",
            ),
            "dotnet": (
                "public enum InterfaceMode : uint",
                "long? Gravity",
                "InterfaceRoutingPolicy? Routing",
            ),
            "go": (
                "type InterfaceMode uint32",
                "Gravity *int64",
                "Routing *InterfaceRoutingPolicy",
            ),
            "swift": (
                "public enum InterfaceMode: UInt32, Sendable",
                "case `internal` = 7",
                "public let gravity: Int64?",
                "case attachInterface(config: InterfaceConfig, routing: InterfaceRoutingPolicy?)",
            ),
            "kotlin": (
                "enum class InterfaceMode(val rawValue: Int)",
                "val gravity: Long?",
                "val routing: InterfaceRoutingPolicy?",
            ),
            "julia": (
                "@enum InterfaceMode::UInt32",
                "gravity::Union{Nothing,Int64}",
                "routing::Union{Nothing,InterfaceRoutingPolicy}",
            ),
        }
        for language, fragments in expected.items():
            with self.subTest(language=language):
                for fragment in fragments:
                    self.assertIn(fragment, projections[language])

    def test_records_arrays_optionals_and_nested_unions_project_exactly(self):
        schema = canonical_schema()
        schema["records"].append(
            {
                "name": "ProjectionProbe",
                "fields": [
                    {
                        "name": "handlers",
                        "type": {"array": "RequestHandlerConfig"},
                    },
                    {"name": "note", "type": "string", "optional": True},
                    {"name": "destination", "type": "DestinationHash"},
                    {"name": "count", "type": "u32"},
                    {"name": "offset", "type": "i16"},
                ],
            }
        )
        schema["unions"].append(
            {
                "name": "NestedProbe",
                "cases": [
                    {
                        "name": "Outcome",
                        "value": 1,
                        "fields": [
                            {"name": "outcome", "type": "CommandOutcome"},
                            {
                                "name": "destinations",
                                "type": {"array": "DestinationName"},
                            },
                        ],
                    }
                ],
            }
        )
        GENERATOR.validate(schema)
        projections = {
            "typescript": GENERATOR.ts_output(schema),
            "c": GENERATOR.c_output(schema),
            "python": GENERATOR.python_output(schema),
            "dotnet": GENERATOR.dotnet_output(schema),
            "go": GENERATOR.go_output(schema),
            "swift": GENERATOR.swift_output(schema),
            "kotlin": GENERATOR.kotlin_output(schema),
            "julia": GENERATOR.julia_output(schema),
        }
        expected = {
            "typescript": (
                "readonly handlers: readonly RequestHandlerConfig[];",
                "readonly note?: string;",
                "readonly outcome: CommandOutcome;",
                "readonly destinations: readonly DestinationName[];",
            ),
            "c": (
                "const PrnsRequestHandlerConfig *handlers;",
                "size_t handler_count;",
                "uint8_t has_note;",
                "PrnsStringView note;",
                "int16_t offset;",
            ),
            "python": (
                "handlers: tuple[RequestHandlerConfig, ...]",
                "note: str | None",
                "outcome: CommandOutcome",
                "destinations: tuple[DestinationName, ...]",
            ),
            "dotnet": (
                "ImmutableArray<RequestHandlerConfig> Handlers",
                "string? Note",
                "CommandOutcome Outcome",
                "ImmutableArray<DestinationName> Destinations",
            ),
            "go": (
                "Handlers []RequestHandlerConfig",
                "Note *string",
                "Outcome CommandOutcome",
                "Destinations []DestinationName",
            ),
            "swift": (
                "public let handlers: [RequestHandlerConfig]",
                "public let note: String?",
                "case outcome(outcome: CommandOutcome, destinations: [DestinationName])",
            ),
            "kotlin": (
                "val handlers: List<RequestHandlerConfig>",
                "val note: String?",
                "val outcome: CommandOutcome",
                "val destinations: List<DestinationName>",
            ),
            "julia": (
                "handlers::Vector{RequestHandlerConfig}",
                "note::Union{Nothing,String}",
                "outcome::CommandOutcome",
                "destinations::Vector{DestinationName}",
            ),
        }
        for language, fragments in expected.items():
            with self.subTest(language=language):
                for fragment in fragments:
                    self.assertIn(fragment, projections[language])
        self.assertIn("PrnsByteView destination;", projections["c"])
        self.assertIn("uint32_t count;", projections["c"])
        self.assertLess(
            projections["julia"].index("abstract type Bitrate end"),
            projections["julia"].index("struct InterfaceConfigTcpClient"),
        )


if __name__ == "__main__":
    unittest.main()
