import assert from "node:assert/strict";
import { test } from "node:test";

import { BluetoothInterface, Tag } from "personal-rns/browser";
import { writeBluetoothValue } from "../dist/browser/bluetooth/gatt.js";

const SERVICE_UUID = "37145b00-442d-4a94-917f-8f42c5da28e3";
const CONTROL_UUID = "37145b00-442d-4a94-917f-8f42c5da28e7";
const DATA_UUID = "37145b00-442d-4a94-917f-8f42c5da28e8";
const HELLO = 0xa1;
const WELCOME = 0xa2;
const FRAGMENT = 0xaf;
const PEER_IDENTITY = new Uint8Array([
  0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
  0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x10,
]);
const INTERFACE_ID = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);

test("Web Bluetooth opens the native Prns GATT contract in both directions", async () => {
  const control = new FakeCharacteristic();
  const data = new FakeCharacteristic();
  const service = new FakeService(control, data);
  const device = new FakeDevice(service);
  const bluetooth = new FakeBluetooth(device);
  const restoreNavigator = replaceNavigator(bluetooth);
  const host = new BluetoothHost([
    { bytes: new Uint8Array([0x31, 0x32, 0x33]) },
  ]);
  control.onWrite = (bytes) => {
    if (bytes[0] === HELLO) {
      queueMicrotask(() => control.notify(new Uint8Array([WELCOME])));
    }
  };

  try {
    const connected = await new BluetoothInterface(host).connect();
    assert.equal(connected.tag, "Connected");
    const session = connected.data;

    assert.deepEqual(bluetooth.requestOptions, {
      filters: [{ services: [SERVICE_UUID] }],
      optionalServices: [SERVICE_UUID],
    });
    assert.deepEqual(service.requested, [CONTROL_UUID, DATA_UUID]);
    assert.equal(control.notificationsStarted, 1);
    assert.equal(data.notificationsStarted, 1);
    assert.deepEqual(control.responseWrites, [[HELLO]]);
    assert.deepEqual(control.commandWrites, []);
    assert.deepEqual(host.registrations, [
      {
        interfaceName: "bluetooth",
        supervisorKind: "bluetooth-auto",
        kind: "bluetooth-peer",
        channelTag: PEER_IDENTITY,
        bitrateBps: 700_000,
        hardwareMtu: 500,
      },
    ]);
    assert.equal(session.status.tag, "Active");
    assert.deepEqual(session.interfaceId, INTERFACE_ID);

    data.notify(new Uint8Array([FRAGMENT, 0x41, 0x42]));
    await waitUntil(() => host.inbound.length === 1);
    assert.deepEqual(host.inbound, [[0x41, 0x42]]);

    await waitUntil(() => data.responseWrites.length === 1);
    assert.deepEqual(data.responseWrites, [[FRAGMENT, 0x31, 0x32, 0x33]]);

    device.disconnectFromPeer();
    await waitUntil(() => host.deactivations.length === 1);
    assert.equal(session.status.tag, "Failed");
    assert.equal(session.status.data.tag, "Disconnected");
    data.notify(new Uint8Array([FRAGMENT, 0x61]));
    assert.deepEqual(host.inbound, [[0x41, 0x42]]);
    assert.equal((await session.close()).tag, "Closed");
  } finally {
    restoreNavigator();
  }
});

test("Web Bluetooth reports a GATT disconnect during the handshake", async () => {
  const control = new FakeCharacteristic();
  const data = new FakeCharacteristic();
  const service = new FakeService(control, data);
  const device = new FakeDevice(service);
  const restoreNavigator = replaceNavigator(new FakeBluetooth(device));
  const host = new BluetoothHost();
  control.onWrite = () => device.disconnectFromPeer();

  try {
    const connected = await new BluetoothInterface(host).connect();
    assert.deepEqual(connected, {
      tag: "ConnectionFailed",
      data: {
        interface: "bluetooth",
        stage: "Handshake",
        detail: "Bluetooth GATT connection closed during the handshake",
      },
    });
    assert.equal(device.gatt.connected, false);
    assert.deepEqual(host.registrations, []);
  } finally {
    restoreNavigator();
  }
});

test("Web Bluetooth carries packet fragments over the control characteristic fallback", async () => {
  const control = new FakeCharacteristic();
  const service = new FakeService(control);
  const device = new FakeDevice(service);
  const restoreNavigator = replaceNavigator(new FakeBluetooth(device));
  const host = new BluetoothHost();
  control.onWrite = (bytes) => {
    if (bytes[0] === HELLO) {
      queueMicrotask(() => control.notify(new Uint8Array([WELCOME])));
    }
  };

  try {
    const connected = await new BluetoothInterface(host).connect();
    assert.equal(connected.tag, "Connected");
    assert.equal(host.controlDecodes, 1);

    control.notify(new Uint8Array([FRAGMENT, 0x51, 0x52, 0x53]));
    await waitUntil(() => host.inbound.length === 1);
    assert.deepEqual(host.inbound, [[0x51, 0x52, 0x53]]);
    assert.equal(host.controlDecodes, 1);

    assert.equal((await connected.data.close()).tag, "Closed");
    assert.equal(device.gatt.connected, false);
    assert.equal(host.deactivations.length, 1);
  } finally {
    restoreNavigator();
  }
});

test("Web Bluetooth preserves data-characteristic discovery failures", async () => {
  const control = new FakeCharacteristic();
  const discoveryFailure = new DOMException("adapter disappeared", "NetworkError");
  const service = new FakeService(control, undefined, discoveryFailure);
  const device = new FakeDevice(service);
  const restoreNavigator = replaceNavigator(new FakeBluetooth(device));
  const host = new BluetoothHost();

  try {
    const connected = await new BluetoothInterface(host).connect();
    assert.deepEqual(connected, {
      tag: "ConnectionFailed",
      data: {
        interface: "bluetooth",
        stage: "ServiceDiscovery",
        detail: "NetworkError: adapter disappeared",
      },
    });
    assert.equal(device.gatt.connected, false);
    assert.deepEqual(host.registrations, []);
  } finally {
    restoreNavigator();
  }
});

test("Web Bluetooth sleeps while idle and wakes for runtime activity", async () => {
  const control = new FakeCharacteristic();
  const data = new FakeCharacteristic();
  const service = new FakeService(control, data);
  const device = new FakeDevice(service);
  const restoreNavigator = replaceNavigator(new FakeBluetooth(device));
  const host = new BluetoothHost();
  control.onWrite = (bytes) => {
    if (bytes[0] === HELLO) {
      queueMicrotask(() => control.notify(new Uint8Array([WELCOME])));
    }
  };

  try {
    const connected = await new BluetoothInterface(host).connect();
    assert.equal(connected.tag, "Connected");
    assert.equal(host.outboundTakes, 1);

    await new Promise((resolve) => setTimeout(resolve, 75));
    assert.equal(host.outboundTakes, 1);

    host.queueOutbound({ bytes: new Uint8Array([0x71, 0x72]) });
    await waitUntil(() => data.responseWrites.length === 1);
    assert.deepEqual(data.responseWrites, [[FRAGMENT, 0x71, 0x72]]);
    assert.equal(host.outboundTakes, 3);
    assert.equal((await connected.data.close()).tag, "Closed");
  } finally {
    restoreNavigator();
  }
});

test("GATT writes honor the characteristic's declared write properties", async () => {
  const acknowledged = new FakeCharacteristic({
    write: true,
    writeWithoutResponse: true,
  });
  assert.equal(
    (await writeBluetoothValue(acknowledged, new Uint8Array([1, 2]))).tag,
    "Written",
  );
  assert.deepEqual(acknowledged.responseWrites, [[1, 2]]);
  assert.deepEqual(acknowledged.commandWrites, []);

  const unacknowledged = new FakeCharacteristic({
    write: false,
    writeWithoutResponse: true,
  });
  assert.equal(
    (await writeBluetoothValue(unacknowledged, new Uint8Array([3, 4]))).tag,
    "Written",
  );
  assert.deepEqual(unacknowledged.responseWrites, []);
  assert.deepEqual(unacknowledged.commandWrites, [[3, 4]]);

  const readOnly = new FakeCharacteristic({
    write: false,
    writeWithoutResponse: false,
  });
  const rejected = await writeBluetoothValue(readOnly, new Uint8Array([5]));
  assert.equal(rejected.tag, "TransferFailed");
  assert.match(rejected.data.detail, /does not support writes/);
});

class BluetoothHost {
  registrations = [];
  deactivations = [];
  inbound = [];
  controlDecodes = 0;
  outboundTakes = 0;
  #outbound;
  #outboundWaiters = [];

  constructor(outbound = []) {
    this.#outbound = outbound;
  }

  bluetoothIdentityReadiness() {
    return Tag("Ready");
  }

  runtimeReadiness() {
    return Tag("Ready");
  }

  bluetoothServiceUuid() {
    return SERVICE_UUID;
  }

  bluetoothControlUuid() {
    return CONTROL_UUID;
  }

  bluetoothDataUuid() {
    return DATA_UUID;
  }

  bluetoothBitrateBps() {
    return 700_000;
  }

  bluetoothHardwareMtu() {
    return 500;
  }

  bluetoothDialerHello() {
    return new Uint8Array([HELLO]);
  }

  bluetoothDecodeControl(bytes) {
    this.controlDecodes += 1;
    if (bytes[0] !== WELCOME) {
      throw new Error("not a Bluetooth control frame");
    }
    return { type: "welcome", identity: PEER_IDENTITY };
  }

  bluetoothDataFragments(packet) {
    return [new Uint8Array([FRAGMENT, ...packet])];
  }

  createBluetoothReassembler() {
    return {
      absorb(bytes) {
        if (bytes[0] !== FRAGMENT) {
          throw new Error("not a Bluetooth data fragment");
        }
        return bytes.slice(1);
      },
    };
  }

  registerInterface(registration) {
    this.registrations.push(registration);
    return Tag("Registered", INTERFACE_ID);
  }

  deactivateInterface(id) {
    this.deactivations.push(new Uint8Array(id));
    this.#resolveOutboundWaiters(Tag("InterfaceDetached"));
    return Tag("Detached");
  }

  ingest(_id, bytes) {
    this.inbound.push([...bytes]);
    return Tag("Accepted");
  }

  takeOutboundFor() {
    this.outboundTakes += 1;
    const outbound = this.#outbound;
    this.#outbound = [];
    return Tag("Outbound", outbound);
  }

  waitForOutboundActivity() {
    return new Promise((resolve) => {
      this.#outboundWaiters.push(resolve);
    });
  }

  queueOutbound(frame) {
    this.#outbound.push(frame);
    this.#resolveOutboundWaiters(Tag("RuntimeAdvanced"));
  }

  #resolveOutboundWaiters(outcome) {
    const waiters = this.#outboundWaiters;
    this.#outboundWaiters = [];
    for (const resolve of waiters) {
      resolve(outcome);
    }
  }
}

class FakeBluetooth {
  requestOptions;
  #device;

  constructor(device) {
    this.#device = device;
  }

  async requestDevice(options) {
    this.requestOptions = options;
    return this.#device;
  }
}

class FakeDevice extends EventTarget {
  gatt;

  constructor(service) {
    super();
    this.gatt = new FakeGattServer(this, service);
  }

  disconnectFromPeer() {
    this.gatt.connected = false;
    this.dispatchEvent(new Event("gattserverdisconnected"));
  }
}

class FakeGattServer {
  connected = false;
  device;
  #service;

  constructor(device, service) {
    this.device = device;
    this.#service = service;
  }

  async connect() {
    this.connected = true;
    return this;
  }

  disconnect() {
    if (!this.connected) {
      return;
    }
    this.connected = false;
    this.device.dispatchEvent(new Event("gattserverdisconnected"));
  }

  async getPrimaryService(uuid) {
    assert.equal(uuid, SERVICE_UUID);
    return this.#service;
  }
}

class FakeService {
  requested = [];
  #control;
  #data;
  #dataFailure;

  constructor(control, data, dataFailure) {
    this.#control = control;
    this.#data = data;
    this.#dataFailure = dataFailure;
  }

  async getCharacteristic(uuid) {
    this.requested.push(uuid);
    if (uuid === CONTROL_UUID) {
      return this.#control;
    }
    if (uuid === DATA_UUID && this.#data) {
      return this.#data;
    }
    if (uuid === DATA_UUID && this.#dataFailure) {
      throw this.#dataFailure;
    }
    throw new DOMException(`missing characteristic ${uuid}`, "NotFoundError");
  }
}

class FakeCharacteristic extends EventTarget {
  value;
  notificationsStarted = 0;
  responseWrites = [];
  commandWrites = [];
  onWrite;
  properties;

  constructor(properties = {}) {
    super();
    this.properties = {
      write: properties.write ?? true,
      writeWithoutResponse: properties.writeWithoutResponse ?? true,
      notify: true,
      indicate: false,
    };
  }

  async startNotifications() {
    this.notificationsStarted += 1;
    return this;
  }

  async writeValueWithResponse(value) {
    const bytes = bytesFrom(value);
    this.responseWrites.push([...bytes]);
    this.onWrite?.(bytes);
  }

  async writeValueWithoutResponse(value) {
    const bytes = bytesFrom(value);
    this.commandWrites.push([...bytes]);
    this.onWrite?.(bytes);
  }

  notify(bytes) {
    const copied = new Uint8Array(bytes);
    this.value = new DataView(copied.buffer);
    this.dispatchEvent(new Event("characteristicvaluechanged"));
  }
}

function bytesFrom(value) {
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }
  return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
}

function replaceNavigator(bluetooth) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { bluetooth },
  });
  return () => {
    if (previous) {
      Object.defineProperty(globalThis, "navigator", previous);
    } else {
      delete globalThis.navigator;
    }
  };
}

async function waitUntil(ready, timeoutMs = 1_000) {
  const deadline = Date.now() + timeoutMs;
  while (!ready()) {
    assert.ok(Date.now() < deadline, "timed out waiting for Bluetooth state");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}
