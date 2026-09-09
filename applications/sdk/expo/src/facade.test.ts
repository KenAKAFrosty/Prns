import * as Bindings from "@prns-internal/native-bindings";
import type { FfiConverter } from "@ubjs/core";
import {
  createDevelopmentRuntime,
  NativeContractMismatchError,
  NativeStoragePreparationError,
} from "./facade";
import type { PrnsAppNativeModule } from "./native";

// Codecs use generated field layouts. This supplies only the player's string
// primitives; domain calls are injected independently and never touch JSI.
jest.mock("../../../prns/native-composition/bindings/typescript/prns_app-ffi", () => ({
  __esModule: true,
  default: () => ({
    ubrn_uniffi_internal_fn_func_ffi__string_to_byte_length: (value: string) =>
      new TextEncoder().encode(value).length,
    ubrn_uniffi_internal_fn_func_ffi__read_string_from_buffer: (
      buffer: { arrayBuffer: ArrayBuffer },
      offset: number,
      length: number,
    ) => new TextDecoder().decode(new Uint8Array(buffer.arrayBuffer, offset, length)),
  }),
}));

const codecs = Bindings.bindingModule.converters;
function encode<T>(codec: FfiConverter<Uint8Array, T>, value: T): number[] {
  return Array.from(codec.lower(value, (size) => new Uint8Array(size)));
}
const snapshot = (): Bindings.DevelopmentNodeSnapshot => ({
  contractFingerprint: Bindings.NATIVE_CONTRACT_FINGERPRINT,
  revision: 18_446_744_073_709_551_615n,
  generationId: 9_007_199_254_740_993n,
  runtime: Bindings.DevelopmentNodeRuntime.Stopped,
  primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
  localHost: Bindings.LocalHostState.Stopped.new({ lastStartFailure: undefined }),
  lxmf: { state: Bindings.LxmfHealthState.Stopped, inboundOverflowCount: 0n },
  pairing: Bindings.RemoteControlPairingState.Searching.new(),
  pairingCandidates: [],
  pairedTargets: [],
});
function nativeModule(overrides: Partial<PrnsAppNativeModule> = {}): PrnsAppNativeModule {
  return {
    prepareStorage: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeNativeStoragePreparationOutcome,
        Bindings.NativeStoragePreparationOutcome.Prepared.new(),
      ),
    ),
    inspectIdentity: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypePrimaryIdentityState,
        Bindings.PrimaryIdentityState.Missing.new(),
      ),
    ),
    createGeneratedIdentity: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeIdentityCreationOutcome,
        Bindings.IdentityCreationOutcome.AlreadyExists.new(),
      ),
    ),
    createImportedIdentity: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeIdentityCreationOutcome,
        Bindings.IdentityCreationOutcome.AlreadyExists.new(),
      ),
    ),
    start: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeDevelopmentNodeStartOutcome,
        Bindings.DevelopmentNodeStartOutcome.Started.new({ snapshot: snapshot() }),
      ),
    ),
    stop: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeDevelopmentNodeStopOutcome,
        Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
      ),
    ),
    reset: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeDevelopmentNodeStopOutcome,
        Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
      ),
    ),
    prepareOutbound: jest.fn(async () => undefined),
    ...overrides,
  };
}
function setup(overrides: Partial<typeof Bindings> = {}, native = nativeModule()) {
  const api = {
    ...Bindings,
    bindingContract: jest.fn(() => ({
      app: Bindings.NATIVE_CONTRACT_FINGERPRINT,
      host: Bindings.HOST_CONTRACT_FINGERPRINT,
    })),
    readSnapshot: jest.fn(async () => snapshot()),
    listContacts: jest.fn(async () => Bindings.ContactListOutcome.Listed.new({ contacts: [] })),
    ...overrides,
  };
  const load = jest.fn(async () => api);
  return { api, load, native, runtime: createDevelopmentRuntime(() => native, load) };
}

test("pure value imports and facade construction do not install a player", () => {
  const { load } = setup();
  expect(Bindings.PrimaryIdentityState.Missing.new().tag).toBe("Missing");
  expect(load).not.toHaveBeenCalled();
});

test("an unavailable native capability rejects before loading the player", async () => {
  const load = jest.fn(async () => Bindings);
  const runtime = createDevelopmentRuntime(() => {
    throw new Error("native unavailable");
  }, load);
  await expect(runtime.readDevelopmentNodeSnapshot()).rejects.toThrow("native unavailable");
  expect(load).not.toHaveBeenCalled();
});

test("checks both semantic contracts once and uses generated startup codecs", async () => {
  const { runtime, api, native, load } = setup();
  const input = { developmentTcpTarget: "192.0.2.1:4242" };
  const started = await runtime.startDevelopmentNode(input);
  const current = await runtime.readDevelopmentNodeSnapshot();
  expect(started).toEqual(
    Bindings.DevelopmentNodeStartOutcome.Started.new({ snapshot: snapshot() }),
  );
  expect(current.revision).toBe(18_446_744_073_709_551_615n);
  expect(native.start).toHaveBeenCalledWith(
    encode(codecs.FfiConverterTypeDevelopmentNodeStartInput, input),
  );
  expect(api.bindingContract).toHaveBeenCalledTimes(1);
  expect(load).toHaveBeenCalledTimes(1);
});

test.each(["app", "host"] as const)(
  "refuses a stale %s contract and allows recovery",
  async (field) => {
    let stale = true;
    const bindingContract = jest.fn(() => ({
      app: Bindings.NATIVE_CONTRACT_FINGERPRINT,
      host: Bindings.HOST_CONTRACT_FINGERPRINT,
      ...(stale ? { [field]: "stale" } : {}),
    }));
    const { runtime, api } = setup({ bindingContract });
    await expect(runtime.readDevelopmentNodeSnapshot()).rejects.toBeInstanceOf(
      NativeContractMismatchError,
    );
    expect(api.readSnapshot).not.toHaveBeenCalled();
    stale = false;
    await expect(runtime.readDevelopmentNodeSnapshot()).resolves.toEqual(snapshot());
  },
);

test("identity bytes use the native lifecycle boundary and the generated bounded preview", async () => {
  const identity = new Uint8Array(64).fill(0x42);
  const identityHash = new Uint8Array(16).fill(0x43);
  const previewIdentityImport = jest.fn(() =>
    Bindings.IdentityImportPreviewOutcome.Valid.new({ identityHash }),
  );
  const native = nativeModule({
    createImportedIdentity: jest.fn(async () =>
      encode(
        codecs.FfiConverterTypeIdentityCreationOutcome,
        Bindings.IdentityCreationOutcome.Created.new({ identityHash }),
      ),
    ),
  });
  const { runtime } = setup({ previewIdentityImport }, native);
  expect(await runtime.previewIdentityImport(identity)).toEqual(
    Bindings.IdentityImportPreviewOutcome.Valid.new({ identityHash }),
  );
  expect(await runtime.createImportedIdentity(identity)).toEqual(
    Bindings.IdentityCreationOutcome.Created.new({ identityHash }),
  );
  expect(previewIdentityImport).toHaveBeenCalledWith(identity);
  expect(native.createImportedIdentity).toHaveBeenCalledWith(Array.from(identity));
});

test("prepares offline storage once, retries failed preparation, and prepares again after reset", async () => {
  let available = false;
  const prepareStorage = jest.fn(async () =>
    encode(
      codecs.FfiConverterTypeNativeStoragePreparationOutcome,
      available
        ? Bindings.NativeStoragePreparationOutcome.Prepared.new()
        : Bindings.NativeStoragePreparationOutcome.DevelopmentResetRequired.new({
            reason: "unsupported store",
          }),
    ),
  );
  const { runtime, api } = setup({}, nativeModule({ prepareStorage }));
  await expect(runtime.listContacts()).rejects.toBeInstanceOf(NativeStoragePreparationError);
  expect(api.listContacts).not.toHaveBeenCalled();
  available = true;
  await Promise.all([runtime.listContacts(), runtime.listContacts()]);
  expect(prepareStorage).toHaveBeenCalledTimes(2);
  await runtime.resetDevelopmentData();
  await runtime.listContacts();
  expect(prepareStorage).toHaveBeenCalledTimes(3);
});

test("passes optional fields, bytes, exact integers, and results unchanged", async () => {
  const destination = new Uint8Array(16).fill(0x44);
  const accepted = Bindings.RetryLxmfMessageOutcome.Accepted.new({
    localRecordId: 18_446_744_073_709_551_615n,
  });
  const retryLxmfMessage = jest.fn(async () => accepted);
  const createManualContact = jest.fn(async () => Bindings.ContactMutationOutcome.NotFound.new());
  const { runtime, native } = setup({ retryLxmfMessage, createManualContact });
  await runtime.createManualContact(destination, undefined, undefined);
  const result = await runtime.retryLxmfMessage(18_446_744_073_709_551_615n);
  expect(createManualContact).toHaveBeenCalledWith(
    { destination, identity: undefined, alias: undefined },
    undefined,
  );
  expect(retryLxmfMessage).toHaveBeenCalledWith(
    { localRecordId: 18_446_744_073_709_551_615n },
    undefined,
  );
  expect(result).toBe(accepted);
  expect(native.prepareOutbound).toHaveBeenCalledTimes(1);
});

test("performs preflight for exactly the six outbound calls and never repeats a failed submission", async () => {
  const failure = new Error("submission interrupted");
  const submitted = jest.fn(async () => {
    throw failure;
  });
  const { runtime, native } = setup({
    initiatePairing: submitted,
    describeTarget: submitted,
    announceTarget: submitted,
    retryLxmfMessage: submitted,
    announceLxmf: submitted,
    sendDirectText: submitted,
  });
  const targetIdentityFingerprint = new Uint8Array(16);
  for (const run of [
    () =>
      runtime.initiateRemoteControlPairing({
        candidateId: "candidate",
        invitationCode: "1234ABCD",
      }),
    () => runtime.describeRemoteControlTarget({ targetIdentityFingerprint }),
    () => runtime.announceRemoteControlTarget({ targetIdentityFingerprint }),
    () => runtime.retryLxmfMessage(1n),
    () => runtime.announceLxmf(),
    () =>
      runtime.sendDirectText({
        destination: targetIdentityFingerprint,
        title: "",
        content: "hello",
      }),
  ])
    await expect(run()).rejects.toBe(failure);
  expect(submitted).toHaveBeenCalledTimes(6);
  expect(native.prepareOutbound).toHaveBeenCalledTimes(6);
});

test("passes cancellation into generated futures and skips a cancelled preflight submission", async () => {
  const controller = new AbortController();
  const { runtime, api } = setup();
  await runtime.readDevelopmentNodeSnapshot(controller.signal);
  expect(api.readSnapshot).toHaveBeenCalledWith({ signal: controller.signal });
  const announceTarget = jest.fn(async () => Bindings.RemoteControlAnnounceOutcome.Busy.new());
  const native = nativeModule({ prepareOutbound: jest.fn(async () => controller.abort()) });
  const outbound = setup({ announceTarget }, native).runtime;
  await expect(
    outbound.announceRemoteControlTarget(
      { targetIdentityFingerprint: new Uint8Array(16) },
      controller.signal,
    ),
  ).rejects.toBeDefined();
  expect(announceTarget).not.toHaveBeenCalled();
});

test("generated snapshot codecs preserve canonical host brands, safe counters, and exact u64 values", () => {
  const { destinationHash, identityHash, interfaceId } =
    jest.requireActual<typeof import("personal-rns/contract")>("personal-rns/contract");
  const maximum = 18_446_744_073_709_551_615n;
  const host: import("personal-rns/contract").HostSnapshot = {
    revision: maximum,
    backend: {
      backend: "Native",
      capabilities: ["Bluetooth"],
      interfaceKinds: ["AutomaticBluetoothLe"],
    },
    interfaces: [
      {
        interfaceId: interfaceId(new Uint8Array(8).fill(1)),
        name: "Bluetooth",
        kind: "AutomaticBluetoothLe",
        health: "Connected",
        rxBytes: maximum,
        txBytes: 9_007_199_254_740_992n,
        rxBps: 7,
        txBps: 8,
        routeCount: 1,
        linkCount: 2,
        transportedLinkCount: 3,
      },
    ],
    routes: [
      {
        destination: destinationHash(new Uint8Array(16).fill(2)),
        viaIdentity: identityHash(new Uint8Array(16).fill(3)),
        interfaceId: interfaceId(new Uint8Array(8).fill(1)),
        hops: 1,
        learnedAtMillis: 11,
        lastRouteActivityAtMillis: 12,
        expiresAtMillis: 13,
      },
    ],
    activeLinkCount: 2,
    destinationIdentities: [
      {
        destination: destinationHash(new Uint8Array(16).fill(2)),
        identity: identityHash(new Uint8Array(16).fill(3)),
      },
    ],
    runtime: {
      running: true,
      uptimeMillis: 14,
      interfaceCount: 1,
      onlineInterfaceCount: 1,
      routeCount: 1,
      linkCount: 2,
      transportedLinkCount: 3,
      rxBytes: maximum,
      txBytes: 9_007_199_254_740_992n,
      rxBps: 7,
      txBps: 8,
    },
    persistence: { persistent: true, restored: true, lastFlushCause: "Startup" },
  };
  const source = { ...snapshot(), localHost: Bindings.LocalHostState.Running.new({ host }) };
  const result = codecs.FfiConverterTypeDevelopmentNodeSnapshot.lift(
    Uint8Array.from(encode(codecs.FfiConverterTypeDevelopmentNodeSnapshot, source)),
  );
  expect(result).toEqual(source);
  if (result.localHost.tag !== "Running") throw new Error("missing canonical host");
  expect(result.localHost.inner.host.runtime.uptimeMillis).toBe(14);
  expect(result.localHost.inner.host.runtime.rxBytes).toBe(maximum);
  expect(result.localHost.inner.host.interfaces[0]?.interfaceId).toBeInstanceOf(Uint8Array);
});

test("supports React Native's actual AbortSignal polyfill without modern convenience methods", async () => {
  const { AbortController: NativeAbortController } =
    jest.requireActual<typeof import("abort-controller")>("abort-controller");
  const controller = new NativeAbortController();
  // React Native exposes this older polyfill as the global AbortSignal.
  const signal = controller.signal as AbortSignal;
  expect(signal.throwIfAborted).toBeUndefined();
  const { runtime, api } = setup();
  await runtime.readDevelopmentNodeSnapshot(signal);
  expect(api.readSnapshot).toHaveBeenCalledWith({ signal });
  controller.abort();
  await expect(runtime.readDevelopmentNodeSnapshot(signal)).rejects.toMatchObject({
    name: "AbortError",
  });
  expect(api.readSnapshot).toHaveBeenCalledTimes(1);
});
