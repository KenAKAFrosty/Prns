import assert from "node:assert/strict";
import test from "node:test";

import {
  cancelNrfBootloaderSelection,
  continueNrfBootloaderSelection,
  createNrfDfuSession,
  runNrfSerialDfu,
} from "../src/nrf-serial-dfu.js";

const USB = Object.freeze({ vendorId: 0x2886, productId: 0x0057 });
const KINDS = Object.freeze({
  AwaitingMore: 0,
  RetryRequired: 1,
  FrameAccepted: 2,
  TransferComplete: 3,
});

function contract(entry = "touch-application-or-bootloader") {
  return {
    entry,
    touchApplicationAndBootloaderUsb: { ...USB },
    touchBaudRate: 1_200,
    transferBaudRate: 115_200,
    managedApplication: {
      usb: { vendorId: 0x1209, productId: 0x0001 },
      manufacturer: "Stay Personal",
      product: "Personal Hopspot (T1000-E)",
      serialNumber: "PERSONAL-RNS-T1000E-HOP",
      interfaceNumber: 0,
      request: 0x50,
      value: 0x5052,
      index: 0x4e53,
    },
  };
}

function transition(kind, total, options = {}) {
  return {
    kind,
    writtenBytes: options.writtenBytes ?? total,
    totalBytes: total,
    waitMilliseconds: options.waitMilliseconds ?? 0,
    retryReason: () => options.retryReason ?? 0,
    free() {},
  };
}

function session(total = 4, transitions = [transition(KINDS.TransferComplete, total)]) {
  let retryAttempts = 0;
  return {
    nextFrame() {
      return { bytes: Uint8Array.of(0x01, 0x02), free() {} };
    },
    pushAcknowledgement() {
      const value = transitions.shift();
      if (value instanceof Error) throw value;
      return value;
    },
    retryFrame() {
      retryAttempts += 1;
      if (retryAttempts > 2) throw new Error("three attempts exhausted");
      return { bytes: Uint8Array.of(0x01, 0x02), free() {} };
    },
    free() {},
  };
}

function port(options = {}) {
  const state = {
    baudRates: [],
    closes: 0,
    signals: [],
    writes: [],
    failOpen: options.failOpen ?? false,
    reads: [...(options.reads ?? [Uint8Array.of(0xc0)])],
  };
  const value = {
    readable: null,
    writable: null,
    getInfo: () => ({ usbVendorId: USB.vendorId, usbProductId: USB.productId }),
    async open(serialOptions) {
      if (state.failOpen) throw new Error("port is absent");
      state.baudRates.push(serialOptions.baudRate);
      value.readable = {
        getReader() {
          return {
            async read() {
              const next = state.reads.shift();
              if (next instanceof Error) throw next;
              return next === undefined
                ? { done: true, value: undefined }
                : { done: false, value: next };
            },
            async cancel() {},
            releaseLock() {},
          };
        },
      };
      value.writable = {
        getWriter() {
          return {
            async write(bytes) {
              state.writes.push(Uint8Array.from(bytes));
              await options.onWrite?.();
            },
            releaseLock() {},
          };
        },
      };
    },
    async setSignals(signals) {
      state.signals.push(signals);
    },
    async close() {
      state.closes += 1;
      value.readable = null;
      value.writable = null;
    },
    state,
  };
  return value;
}

function serial(selectedPort, options = {}) {
  const listeners = new Map();
  const state = { requestOptions: [], getPortsCalls: 0 };
  return {
    state,
    async requestPort(requestOptions) {
      state.requestOptions.push(requestOptions);
      if (options.requestError) throw options.requestError;
      return options.requestPort?.() ?? selectedPort;
    },
    async getPorts() {
      state.getPortsCalls += 1;
      return options.getPorts?.(state.getPortsCalls) ?? [selectedPort];
    },
    addEventListener(name, listener) {
      listeners.set(name, listener);
    },
    removeEventListener(name, listener) {
      if (listeners.get(name) === listener) listeners.delete(name);
    },
    disconnect(target) {
      listeners.get("disconnect")?.({ target });
    },
  };
}

function dependencies(serialApi, extra = {}) {
  let milliseconds = 0;
  return {
    serial: serialApi,
    environment: { navigator: { serial: serialApi }, isSecureContext: true },
    nowImpl: () => milliseconds,
    sleepImpl: async (duration) => { milliseconds += Math.max(duration, 1); },
    ...extra,
  };
}

function prepared(entry, dfuSession) {
  const application = Uint8Array.of(1, 2, 3, 4);
  return {
    serialFilters: [{ usbVendorId: USB.vendorId, usbProductId: USB.productId }],
    files: [
      { kind: "dfu-application", bytes: application },
      { kind: "dfu-init-packet", bytes: Uint8Array.of(5) },
    ],
    nrfDfu: {
      contract: contract(entry),
      core: { NrfDfuAcknowledgementTransitionKind: KINDS },
      session: dfuSession,
    },
  };
}

function emitted() {
  const values = [];
  return { values, events: { emit: (event) => values.push(event) } };
}

test.beforeEach(() => cancelNrfBootloaderSelection());
test.afterEach(() => cancelNrfBootloaderSelection());

test("preparation constructs the Rust session from only the verified application and init packet", async () => {
  const seen = {};
  const compatibility = { free() { seen.compatibilityFreed = true; } };
  const core = {
    NrfDfuBankLayout: { Single: 7, Dual: 8 },
    NrfDfuCompatibility: {
      notEnforcedApplication(deviceType, deviceRevision, fwids, bankLayout) {
        Object.assign(seen, {
          deviceType,
          deviceRevision,
          fwids: Array.from(fwids),
          bankLayout,
        });
        return compatibility;
      },
    },
    NrfDfuSession: class {
      constructor(application, initPacket, receivedCompatibility) {
        seen.application = Array.from(application);
        seen.initPacket = Array.from(initPacket);
        seen.receivedCompatibility = receivedCompatibility;
      }
    },
  };
  const dfuContract = {
    ...contract(),
    compatibility: {
      deviceType: 0x0052,
      deviceRevision: 52840,
      softdeviceFwids: [0x0123],
      bankLayout: "single",
    },
  };
  const files = [
    { kind: "dfu-application", bytes: Uint8Array.of(1, 2, 3) },
    { kind: "dfu-init-packet", bytes: Uint8Array.of(4, 5) },
  ];
  const created = await createNrfDfuSession(dfuContract, files, { nrfDfuCore: core });

  assert.equal(created.contract, dfuContract);
  assert.deepEqual(seen, {
    deviceType: 0x0052,
    deviceRevision: 52840,
    fwids: [0x0123],
    bankLayout: 7,
    application: [1, 2, 3],
    initPacket: [4, 5],
    receivedCompatibility: compatibility,
    compatibilityFreed: true,
  });
});

test("stock or bootloader entry uses the exact serial identity and completes through Rust state", async () => {
  const selected = port();
  const serialApi = serial(selected);
  const { events, values } = emitted();
  const result = await runNrfSerialDfu({
    prepared: prepared("touch-application-or-bootloader", session()),
    events,
    dependencies: dependencies(serialApi),
    isCancelled: () => false,
  });

  assert.deepEqual(result, { success: true });
  assert.deepEqual(serialApi.state.requestOptions, [{
    filters: [{ usbVendorId: USB.vendorId, usbProductId: USB.productId }],
  }]);
  assert.deepEqual(selected.state.baudRates, [1_200, 115_200, 115_200]);
  assert.deepEqual(selected.state.signals, [
    { dataTerminalReady: true },
    { dataTerminalReady: true },
    { dataTerminalReady: true },
  ]);
  assert.deepEqual(values.map(({ phase }) => phase), [
    "requesting_port",
    "connecting",
    "verifying_target",
    "writing",
    "writing",
    "verifying_flash",
    "resetting",
    "success",
  ]);
});

test("managed Personal Hopspot entry uses exact WebUSB control then a bounded serial continuation", async () => {
  const selected = port();
  const serialApi = serial(selected, {
    getPorts: () => [],
    requestPort: () => selected,
  });
  const usbState = { filters: null, claimed: null, control: null, closed: false };
  const usbDevice = {
    vendorId: 0x1209,
    productId: 0x0001,
    manufacturerName: "Stay Personal",
    productName: "Personal Hopspot (T1000-E)",
    serialNumber: "PERSONAL-RNS-T1000E-HOP",
    configurations: [{ configurationValue: 1 }],
    configuration: null,
    opened: false,
    async open() { this.opened = true; },
    async selectConfiguration(configurationValue) {
      assert.equal(configurationValue, 1);
      this.configuration = { interfaces: [{ interfaceNumber: 0 }] };
    },
    async claimInterface(interfaceNumber) { usbState.claimed = interfaceNumber; },
    async controlTransferOut(control) {
      usbState.control = control;
      return { status: "ok", bytesWritten: 0 };
    },
    async close() { this.opened = false; usbState.closed = true; },
  };
  const usb = {
    async requestDevice(options) {
      usbState.filters = options;
      return usbDevice;
    },
  };
  const { events, values } = emitted();
  let releaseContinuation;
  const awaiting = new Promise((resolve) => { releaseContinuation = resolve; });
  const notifyingEvents = {
    emit(event) {
      events.emit(event);
      if (event.phase === "awaiting_bootloader_port") releaseContinuation();
    },
  };
  const operation = runNrfSerialDfu({
    prepared: prepared("managed-application", session()),
    events: notifyingEvents,
    dependencies: dependencies(serialApi, { usb }),
    isCancelled: () => false,
  });
  await awaiting;
  assert.deepEqual(await continueNrfBootloaderSelection(), { selected: true });
  assert.deepEqual(await operation, { success: true });

  assert.deepEqual(usbState.filters, { filters: [{
    vendorId: 0x1209,
    productId: 0x0001,
    serialNumber: "PERSONAL-RNS-T1000E-HOP",
  }] });
  assert.equal(usbState.claimed, 0);
  assert.deepEqual(usbState.control, {
    requestType: "vendor",
    recipient: "device",
    request: 0x50,
    value: 0x5052,
    index: 0x4e53,
  });
  assert.equal(usbState.closed, true);
  assert.equal(values.some(({ phase }) => phase === "awaiting_bootloader_port"), true);
});

test("permission denial and wrong selected identities fail closed before transfer", async () => {
  const denied = Object.assign(new Error("picker cancelled"), { name: "NotFoundError" });
  const deniedSerial = serial(port(), { requestError: denied });
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session()),
      events: emitted().events,
      dependencies: dependencies(deniedSerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "permission_denied",
  );

  const wrong = port();
  wrong.getInfo = () => ({ usbVendorId: USB.vendorId, usbProductId: 0xffff });
  const wrongSerial = serial(wrong);
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session()),
      events: emitted().events,
      dependencies: dependencies(wrongSerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "ambiguous_device",
  );
});

test("ambiguous bootloaders and bounded reconnect timeout fail closed", async () => {
  const selected = port();
  const duplicate = port();
  const ambiguousSerial = serial(selected, { getPorts: () => [selected, duplicate] });
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session()),
      events: emitted().events,
      dependencies: dependencies(ambiguousSerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "ambiguous_device",
  );

  const absent = port({ onWrite: null });
  const absentSerial = serial(absent, {
    getPorts() {
      absent.state.failOpen = true;
      return [];
    },
  });
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session()),
      events: emitted().events,
      dependencies: dependencies(absentSerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "reconnect_timeout",
  );
});

test("device loss, malformed acknowledgements, and retry exhaustion remain distinct", async () => {
  let lossSerial;
  const lost = port({
    async onWrite() {
      lossSerial.disconnect(lost);
      throw new Error("gone");
    },
  });
  lossSerial = serial(lost);
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session()),
      events: emitted().events,
      dependencies: dependencies(lossSerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "device_lost",
  );

  const malformedPort = port();
  const malformedSerial = serial(malformedPort);
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared(
        "touch-application-or-bootloader",
        session(4, [new Error("Rust rejected bytes")]),
      ),
      events: emitted().events,
      dependencies: dependencies(malformedSerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "malformed_acknowledgement",
  );

  const retries = Array.from(
    { length: 3 },
    () => transition(KINDS.RetryRequired, 4, { retryReason: 0 }),
  );
  const retryPort = port({ reads: retries.map(() => Uint8Array.of(0xc0)) });
  const retrySerial = serial(retryPort);
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session(4, retries)),
      events: emitted().events,
      dependencies: dependencies(retrySerial),
      isCancelled: () => false,
    }),
    (error) => error.code === "retries_exhausted",
  );
});

test("cancellation cannot be mistaken for a successful transfer", async () => {
  const selected = port();
  const serialApi = serial(selected);
  await assert.rejects(
    runNrfSerialDfu({
      prepared: prepared("touch-application-or-bootloader", session()),
      events: emitted().events,
      dependencies: dependencies(serialApi),
      isCancelled: () => true,
    }),
    (error) => error.code === "cancelled",
  );
  assert.equal(selected.state.writes.length, 0);
});
