import * as Bindings from "@prns-internal/native-bindings";
import { Effect } from "effect";
import {
  DevelopmentRuntimeConfigurationError,
  DevelopmentRuntimeOperationError,
  DevelopmentRuntimeStartError,
  DevelopmentRuntimeStopError,
  makeEffectDevelopmentRuntime,
  scopedDevelopmentRuntime,
} from "./effects";
import type { DevelopmentNodeSnapshot, DevelopmentRuntime } from "./facade";
function snapshot(revision: bigint): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "prns-app-native/foundation-1/test",
    revision,
    runtime: Bindings.DevelopmentNodeRuntime.Running,
    primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
    localHost: Bindings.LocalHostState.Stopped.new({
      lastStartFailure: undefined,
    }),
    lxmf: { state: Bindings.LxmfHealthState.Stopped, inboundOverflowCount: 0n },
    controllerIdentityFingerprint: undefined,
    pairing: Bindings.RemoteControlPairingState.Searching.new(),
    pairingCandidates: [],
    pairedTargets: [],
    lastAnnouncement: undefined,
    generationId: 0n,
    activeOperation: undefined,
    failure: undefined,
  };
}
function fakeRuntime(overrides: Partial<DevelopmentRuntime> = {}): DevelopmentRuntime {
  return {
    inspectDevelopmentIdentity: jest.fn(async () => Bindings.PrimaryIdentityState.Missing.new()),
    previewIdentityImport: jest.fn(async () =>
      Bindings.IdentityImportPreviewOutcome.InvalidLength.new(),
    ),
    createGeneratedIdentity: jest.fn(async () =>
      Bindings.IdentityCreationOutcome.AlreadyExists.new(),
    ),
    createImportedIdentity: jest.fn(async () =>
      Bindings.IdentityCreationOutcome.AlreadyExists.new(),
    ),
    startDevelopmentNode: jest.fn(async () =>
      Bindings.DevelopmentNodeStartOutcome.Started.new({
        snapshot: snapshot(1n),
      }),
    ),
    readDevelopmentNodeSnapshot: jest.fn(async () => snapshot(1n)),
    initiateRemoteControlPairing: jest.fn(async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    ),
    approveRemoteControlPairing: jest.fn(async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    ),
    rejectRemoteControlPairing: jest.fn(async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    ),
    describeRemoteControlTarget: jest.fn(async () =>
      Bindings.RemoteControlDescribeOutcome.Busy.new(),
    ),
    announceRemoteControlTarget: jest.fn(async () =>
      Bindings.RemoteControlAnnounceOutcome.Busy.new(),
    ),
    saveObservedDestination: jest.fn(async () => Bindings.ContactMutationOutcome.NotObserved.new()),
    createManualContact: jest.fn(async () => Bindings.ContactMutationOutcome.NotFound.new()),
    setContactAlias: jest.fn(async () => Bindings.ContactMutationOutcome.NotFound.new()),
    setContactPinned: jest.fn(async () => Bindings.ContactMutationOutcome.NotFound.new()),
    deleteContact: jest.fn(async () => Bindings.ContactMutationOutcome.NotFound.new()),
    getContact: jest.fn(async () => Bindings.ContactLookupOutcome.NotFound.new()),
    listContacts: jest.fn(async () =>
      Bindings.ContactListOutcome.Listed.new({
        contacts: [],
      }),
    ),
    listLxmfPeers: jest.fn(async () =>
      Bindings.LxmfPeerListOutcome.Listed.new({
        peers: [],
      }),
    ),
    listLxmfMessages: jest.fn(async () =>
      Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [],
      }),
    ),
    retryLxmfMessage: jest.fn(async () => Bindings.RetryLxmfMessageOutcome.NotFound.new()),
    cancelLxmfMessage: jest.fn(async () => Bindings.CancelLxmfMessageOutcome.NotFound.new()),
    announceLxmf: jest.fn(async () => Bindings.AnnounceLxmfOutcome.Announced),
    measureLxmfText: jest.fn(async () =>
      Bindings.MeasureLxmfTextOutcome.Measured.new({
        wireBytes: 113,
        remainingBytes: 318,
      }),
    ),
    sendDirectText: jest.fn(async () =>
      Bindings.SendDirectTextOutcome.Accepted.new({
        localRecordId: 1n,
      }),
    ),
    stopDevelopmentNode: jest.fn(async () => Bindings.DevelopmentNodeStopOutcome.Stopped.new()),
    resetDevelopmentData: jest.fn(async () =>
      Bindings.DevelopmentNodeStopOutcome.AlreadyStopped.new(),
    ),
    ...overrides,
  };
}
describe("Effect development runtime orchestration", () => {
  test("scopes one runtime, ignores stale revisions, and cancels sequential refresh", async () => {
    const revisions = [1n, 0n, 2n, 2n];
    let readIndex = 0;
    const readDevelopmentNodeSnapshot = jest.fn(async () => snapshot(revisions[readIndex++] ?? 2n));
    const stopDevelopmentNode = jest.fn(async () =>
      Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
    );
    const runtime = fakeRuntime({ readDevelopmentNodeSnapshot, stopDevelopmentNode });
    const observed: bigint[] = [];
    const failures: unknown[] = [];
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          yield* scopedDevelopmentRuntime(runtime, {
            refreshIntervalMillis: 50,
            onSnapshot: (current) => observed.push(current.revision),
            onBackgroundFailure: (failure) => failures.push(failure),
          });
          yield* Effect.sleep(190);
        }),
      ),
    );
    expect(observed).toEqual([1n, 2n]);
    expect(failures).toEqual([]);
    expect(runtime.startDevelopmentNode).toHaveBeenCalledWith({ developmentTcpTarget: undefined });
    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
    const readsAfterClose = readDevelopmentNodeSnapshot.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(readsAfterClose);
  });
  test("process ownership survives scope close and remount while refresh fibers remain scoped", async () => {
    const running = snapshot(1n);
    const startDevelopmentNode = jest
      .fn()
      .mockResolvedValueOnce(
        Bindings.DevelopmentNodeStartOutcome.Started.new({
          snapshot: running,
        }),
      )
      .mockResolvedValueOnce(
        Bindings.DevelopmentNodeStartOutcome.AlreadyRunning.new({
          snapshot: running,
        }),
      );
    const readDevelopmentNodeSnapshot = jest.fn(async () => snapshot(2n));
    const stopDevelopmentNode = jest.fn(async () =>
      Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
    );
    const runtime = fakeRuntime({
      startDevelopmentNode,
      readDevelopmentNodeSnapshot,
      stopDevelopmentNode,
    });
    for (let mount = 0; mount < 2; mount += 1) {
      await Effect.runPromise(
        Effect.scoped(
          Effect.gen(function* () {
            yield* scopedDevelopmentRuntime(runtime, {
              nativeLifetime: "process",
              refreshIntervalMillis: 50,
              onSnapshot: () => undefined,
              onBackgroundFailure: () => undefined,
            });
            yield* Effect.sleep(80);
          }),
        ),
      );
      const readsAfterClose = readDevelopmentNodeSnapshot.mock.calls.length;
      await new Promise((resolve) => setTimeout(resolve, 80));
      expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(readsAfterClose);
    }
    expect(startDevelopmentNode).toHaveBeenCalledTimes(2);
    expect(stopDevelopmentNode).not.toHaveBeenCalled();
  });
  test.each([
    [
      "a rejected native promise",
      jest.fn(async () => {
        throw new Error("bridge detached");
      }),
    ],
    [
      "a typed startup failure",
      jest.fn(async () =>
        Bindings.DevelopmentNodeStartOutcome.Failed.new({
          stage: Bindings.DevelopmentNodeFailureStage.Runtime,
          detail: "worker did not start",
        }),
      ),
    ],
  ])("process ownership never stops after %s", async (_label, startDevelopmentNode) => {
    const stopDevelopmentNode = jest.fn(async () =>
      Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
    );
    const runtime = fakeRuntime({ startDevelopmentNode, stopDevelopmentNode });
    await expect(
      Effect.runPromise(
        Effect.flip(
          Effect.scoped(
            scopedDevelopmentRuntime(runtime, {
              nativeLifetime: "process",
              onSnapshot: () => undefined,
              onBackgroundFailure: () => undefined,
            }),
          ),
        ),
      ),
    ).resolves.toBeDefined();
    expect(stopDevelopmentNode).not.toHaveBeenCalled();
  });
  test("forwards the explicit development TCP target only through scoped startup", async () => {
    const runtime = fakeRuntime();
    await Effect.runPromise(
      Effect.scoped(
        scopedDevelopmentRuntime(runtime, {
          developmentTcpTarget: "192.0.2.1:4242",
          onSnapshot: () => undefined,
          onBackgroundFailure: () => undefined,
        }),
      ),
    );
    expect(runtime.startDevelopmentNode).toHaveBeenCalledWith({
      developmentTcpTarget: "192.0.2.1:4242",
    });
  });
  test("keeps the node owned while pausing snapshot reads outside consuming screens", async () => {
    let refreshActive = false;
    const readDevelopmentNodeSnapshot = jest.fn(async () => snapshot(2n));
    const stopDevelopmentNode = jest.fn(async () =>
      Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
    );
    const runtime = fakeRuntime({ readDevelopmentNodeSnapshot, stopDevelopmentNode });
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          yield* scopedDevelopmentRuntime(runtime, {
            refreshIntervalMillis: 50,
            shouldRefresh: () => refreshActive,
            onSnapshot: () => undefined,
            onBackgroundFailure: () => undefined,
          });
          yield* Effect.sleep(120);
          expect(readDevelopmentNodeSnapshot).not.toHaveBeenCalled();
          refreshActive = true;
          yield* Effect.sleep(120);
          expect(readDevelopmentNodeSnapshot).toHaveBeenCalled();
        }),
      ),
    );
    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
  });
  test("stops after a typed startup rejection before failing acquisition", async () => {
    const stopDevelopmentNode = jest.fn(async () =>
      Bindings.DevelopmentNodeStopOutcome.Stopped.new(),
    );
    const runtime = fakeRuntime({
      startDevelopmentNode: jest.fn(async () =>
        Bindings.DevelopmentNodeStartOutcome.Failed.new({
          stage: Bindings.DevelopmentNodeFailureStage.Runtime,
          detail: "worker did not start",
        }),
      ),
      stopDevelopmentNode,
    });
    const failure = await Effect.runPromise(
      Effect.flip(
        Effect.scoped(
          scopedDevelopmentRuntime(runtime, {
            onSnapshot: () => undefined,
            onBackgroundFailure: () => undefined,
          }),
        ),
      ),
    );
    expect(failure).toBeInstanceOf(DevelopmentRuntimeStartError);
    expect(stopDevelopmentNode).toHaveBeenCalledTimes(1);
  });
  test("maps rejected native promises to operation-specific typed errors", async () => {
    const runtime = makeEffectDevelopmentRuntime(
      fakeRuntime({
        readDevelopmentNodeSnapshot: jest.fn(async () => {
          throw new Error("bridge detached");
        }),
      }),
    );
    const failure = await Effect.runPromise(Effect.flip(runtime.readDevelopmentNodeSnapshot));
    expect(failure).toBeInstanceOf(DevelopmentRuntimeOperationError);
    expect(failure.operation).toBe("snapshot");
  });
  test("reports a transient snapshot failure and continues bounded refresh", async () => {
    let readCount = 0;
    const readDevelopmentNodeSnapshot = jest.fn(async () => {
      readCount += 1;
      if (readCount === 1) {
        throw new Error("temporary bridge interruption");
      }
      return snapshot(2n);
    });
    const observed: bigint[] = [];
    const failures: unknown[] = [];
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          yield* scopedDevelopmentRuntime(fakeRuntime({ readDevelopmentNodeSnapshot }), {
            refreshIntervalMillis: 50,
            onSnapshot: (current) => observed.push(current.revision),
            onBackgroundFailure: (failure) => failures.push(failure),
          });
          yield* Effect.sleep(140);
        }),
      ),
    );
    expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(2);
    expect(observed).toEqual([1n, 2n]);
    expect(failures).toHaveLength(1);
    expect(failures[0]).toBeInstanceOf(DevelopmentRuntimeOperationError);
  });
  test("rejects an unbounded refresh cadence before starting native", async () => {
    const runtime = fakeRuntime();
    const failure = await Effect.runPromise(
      Effect.flip(
        Effect.scoped(
          scopedDevelopmentRuntime(runtime, {
            refreshIntervalMillis: 1,
            onSnapshot: () => undefined,
            onBackgroundFailure: () => undefined,
          }),
        ),
      ),
    );
    expect(failure).toBeInstanceOf(DevelopmentRuntimeConfigurationError);
    expect(runtime.startDevelopmentNode).not.toHaveBeenCalled();
  });
  test("reports an incomplete final stop through the required failure sink", async () => {
    const failures: unknown[] = [];
    const runtime = fakeRuntime({
      stopDevelopmentNode: jest.fn(async () =>
        Bindings.DevelopmentNodeStopOutcome.Failed.new({
          stage: Bindings.DevelopmentNodeStopStage.Worker,
          detail: "worker did not join",
        }),
      ),
    });
    await Effect.runPromise(
      Effect.scoped(
        scopedDevelopmentRuntime(runtime, {
          onSnapshot: () => undefined,
          onBackgroundFailure: (failure) => failures.push(failure),
        }),
      ),
    );
    expect(failures).toHaveLength(1);
    expect(failures[0]).toBeInstanceOf(DevelopmentRuntimeStopError);
  });
});

test("closing a process-owned scope aborts its pending generated snapshot waiter", async () => {
  let pendingSignal: AbortSignal | undefined;
  const runtime = fakeRuntime({
    readDevelopmentNodeSnapshot: (signal) =>
      new Promise((_resolve, reject) => {
        pendingSignal = signal;
        signal?.addEventListener("abort", () => reject(signal.reason), { once: true });
      }),
  });
  await Effect.runPromise(
    Effect.scoped(
      Effect.gen(function* () {
        yield* scopedDevelopmentRuntime(runtime, {
          nativeLifetime: "process",
          refreshIntervalMillis: 50,
          onSnapshot: () => undefined,
          onBackgroundFailure: () => undefined,
        });
        yield* Effect.sleep(90);
      }),
    ),
  );
  expect(pendingSignal?.aborted).toBe(true);
  expect(runtime.stopDevelopmentNode).not.toHaveBeenCalled();
});
