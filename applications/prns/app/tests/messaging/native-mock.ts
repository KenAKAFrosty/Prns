// Keep the real generated enums, records, and exported converter. Mock only the
// native image's string bridge, token factory and storage/platform boundaries.
jest.mock("@react-native-async-storage/async-storage", () => ({ __esModule: true, default: {
  getItem: jest.fn(), setItem: jest.fn(), removeItem: jest.fn(),
} }));
jest.mock("../../../bindings/typescript/prns_groups-ffi", () => ({ __esModule: true, default: () => mockImage() }));
jest.mock("../../../bindings/typescript/prns_groups_core-ffi", () => ({ __esModule: true, default: () => mockImage() }));
jest.mock("../../../bindings/typescript/prns_groups_store_redb-ffi", () => ({ __esModule: true, default: () => mockImage() }));
jest.mock("../../../bindings/typescript/prns_groups_service-ffi", () => ({ __esModule: true, default: () => mockImage() }));
jest.mock("@prns/app-bindings", () => {
  const util = require("node:util");
  global.TextEncoder = util.TextEncoder;
  global.TextDecoder = util.TextDecoder;
  const native = jest.requireActual("../../../bindings/typescript/prns_groups");
  return { prns_groups: { ...native, createRequestId: jest.fn(), textPublication: jest.fn() },
    prns_groups_core: jest.requireActual("../../../bindings/typescript/prns_groups_core"),
    prns_groups_service: jest.requireActual("../../../bindings/typescript/prns_groups_service"),
    prns_chat_app: { applicationGroupProfile: jest.fn((maxMembers: number) => ({
      maxMembers, maxRecordBodyBytes: 2054, maxPlaintextBytes: 16384,
      maxWireBytes: 65536, maxSnapshotBytes: 262144, maxEnrollmentFrameBytes: 524288,
    })) } };
});
function mockImage() {
  return {
    ubrn_uniffi_internal_fn_func_ffi__string_to_byte_length: (value: string) => Buffer.byteLength(value),
    ubrn_uniffi_internal_fn_func_ffi__read_string_from_buffer: (value: { arrayBuffer: ArrayBuffer }, offset: number, length: number) =>
      Buffer.from(value.arrayBuffer, offset, length).toString("utf8"),
  };
}
