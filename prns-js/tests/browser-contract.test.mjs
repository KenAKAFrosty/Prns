import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import * as internalContract from "../dist/contract.js";

import {
  HOST_CONTRACT_ABI,
  HOST_SCHEMA_VERSION,
  DESTINATION_HASH_LENGTH,
  PRODUCT_VERSION,
  SAFE_INT_MAX,
  SAFE_INT_MIN,
  Prns,
  Tag,
  balancedLimits,
  match,
} from "personal-rns/browser";

const productVersion = (await readFile("../VERSION", "utf8")).trim();

test("browser subpath exposes the shared release contract and casework", () => {
  assert.equal(HOST_CONTRACT_ABI, 1);
  assert.equal(HOST_SCHEMA_VERSION, 1);
  assert.equal(PRODUCT_VERSION, productVersion);
  assert.deepEqual(balancedLimits(), {
    pendingCommands: 256,
    applicationEvents: 1_024,
    retainedEventBytes: 8 * 1_024 * 1_024,
    diagnostics: 1_024,
  });
  assert.equal(typeof Prns.create, "function");
  assert.equal(
    match(Tag("Ready", 8), {
      Ready: (value) => value,
    }),
    8,
  );
});

test("generated JavaScript contract agrees with language-neutral vectors", async () => {
  const vectors = JSON.parse(
    await readFile("../prns-host/conformance/host-contract-v1.json", "utf8"),
  );
  assert.equal(HOST_CONTRACT_ABI, vectors.abi);
  assert.equal(HOST_SCHEMA_VERSION, vectors.schemaVersion);
  assert.equal(PRODUCT_VERSION, vectors.productVersion);
  assert.equal(DESTINATION_HASH_LENGTH, vectors.fixedBytes.DestinationHash);
  assert.equal(SAFE_INT_MIN, Number(vectors.scalars.safeInt.minimum));
  assert.equal(SAFE_INT_MAX, Number(vectors.scalars.safeInt.maximum));
  assert.deepEqual(balancedLimits(), vectors.limits);
  assert.equal(vectors.integerChecks.safeInt.typescript, "number");
  assert.equal(vectors.integerChecks.safeUint.typescript, "number");
  assert.equal(vectors.integerChecks.u64.typescript, "bigint");
  assert.ok(vectors.integerChecks.u64.accepted.includes("18446744073709551615"));
});

test("generated contract inventories and guards accept exactly their known strings", () => {
  const contracts = [
    [
      "CAPABILITY_NAME_VALUES",
      "isCapabilityName",
      ["Loopback", "TcpClient", "TcpServer", "Udp", "Serial", "Usb", "Bluetooth", "Wifi", "WebSocket", "BrowserRendezvous", "I2p", "Weave"],
    ],
    [
      "LINK_CLOSED_REASON_VALUES",
      "isLinkClosedReason",
      ["Timeout", "PeerClosed", "MalformedRtt"],
    ],
    ["HOST_ROLE_NAME_VALUES", "isHostRoleName", ["Endpoint", "Transport"]],
    [
      "DELIVERY_EVIDENCE_KIND_VALUES",
      "isDeliveryEvidenceKind",
      ["ExplicitProof", "ImplicitProof", "Response"],
    ],
    ["REQUEST_POLICY_VALUES", "isRequestPolicy", ["AllowNone", "AllowAll", "AllowList"]],
    [
      "PERSISTENCE_FLUSH_CAUSE_VALUES",
      "isPersistenceFlushCause",
      ["Startup", "Interval", "RouteChange", "RatchetRotation", "Shutdown"],
    ],
    [
      "PERSISTENCE_FLUSH_TARGET_VALUES",
      "isPersistenceFlushTarget",
      ["RoutingState", "Ratchets"],
    ],
    ["BACKEND_KIND_VALUES", "isBackendKind", ["Native", "Browser", "Cooperative"]],
    [
      "INTERFACE_KIND_VALUES",
      "isInterfaceKind",
      [
        "AutoLan", "TcpClient", "TcpServer", "Udp", "Serial", "Kiss", "Ax25Kiss",
        "RNode", "MultiRNode", "Pipe", "BackboneClient", "BackboneServer", "I2p",
        "Weave", "AutomaticUsb", "AutomaticBluetoothLe", "WebSocketClient",
        "WebSocketServer", "BrowserRendezvous",
      ],
    ],
    [
      "INTERFACE_MODE_VALUES",
      "isInterfaceMode",
      ["Full", "PointToPoint", "AccessPoint", "Roaming", "Boundary", "Gateway", "Internal"],
    ],
    [
      "INTERFACE_HEALTH_VALUES",
      "isInterfaceHealth",
      ["Initializing", "Connected", "Degraded", "Reconnecting", "Failed", "Disconnected", "Disabled", "Unknown"],
    ],
    [
      "DISCOVERY_SCOPE_VALUES",
      "isDiscoveryScope",
      ["Link", "Admin", "Site", "Organization", "Global"],
    ],
    [
      "MULTICAST_ADDRESS_TYPE_VALUES",
      "isMulticastAddressType",
      ["Temporary", "Permanent"],
    ],
    ["SERIAL_DATA_BITS_VALUES", "isSerialDataBits", ["Five", "Six", "Seven", "Eight"]],
    ["SERIAL_PARITY_VALUES", "isSerialParity", ["None", "Even", "Odd"]],
    ["SERIAL_STOP_BITS_VALUES", "isSerialStopBits", ["One", "Two"]],
  ];
  for (const [inventoryName, guardName, expected] of contracts) {
    assert.deepEqual(internalContract[inventoryName], expected);
    for (const value of expected) {
      assert.equal(internalContract[guardName](value), true);
    }
    assert.equal(internalContract[guardName](42), false);
    assert.equal(internalContract[guardName](`Unknown${expected[0]}`), false);
  }
});

test("internal contract-value decoding rejects invalid native values structurally", () => {
  assert.throws(
    () =>
      internalContract.contractValue(
        "backend",
        "Future",
        internalContract.isBackendKind,
      ),
    (error) =>
      error instanceof internalContract.PrnsValidationError &&
      error.code === "InvalidEnum",
  );
});
